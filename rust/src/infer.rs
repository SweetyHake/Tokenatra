use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde_json::json;

use crate::state::{AppState, ProviderConfig};

/// Providers linked into this binary.
pub fn available_providers() -> &'static [&'static str] {
    if cfg!(windows) {
        &["CUDAExecutionProvider", "DmlExecutionProvider", "CPUExecutionProvider"]
    } else if cfg!(target_os = "macos") {
        &["CoreMLExecutionProvider", "CPUExecutionProvider"]
    } else if cfg!(target_os = "linux") {
        &["CUDAExecutionProvider", "ROCMExecutionProvider", "CPUExecutionProvider"]
    } else {
        &["CPUExecutionProvider"]
    }
}

pub struct Session {
    session: ort::session::Session,
    input_name: String,
}

impl Session {
    #[allow(dead_code)]
    pub fn input_name(&self) -> &str {
        &self.input_name
    }

    /// Runs the NCHW input (1x3x1024x1024), returns the raw f32 mask.
    pub fn run(&mut self, tensor: Vec<f32>) -> anyhow::Result<Vec<f32>> {
        let value = ort::value::Tensor::from_array((
            vec![1i64, 3, 1024, 1024],
            tensor,
        ))?;
        let outputs = self
            .session
            .run(ort::inputs![self.input_name.as_str() => value])?;
        let first = &outputs[0];
        // models output f32, f16 (BEN2/RMBG fp16) or f64
        if let Ok(arr) = first.try_extract_array::<f32>() {
            let flat: Vec<f32> = arr.iter().copied().collect();
            return Ok(flat);
        }
        if let Ok(arr) = first.try_extract_array::<half::f16>() {
            let flat: Vec<f32> = arr.iter().map(|v| v.to_f32()).collect();
            return Ok(flat);
        }
        let arr = first.try_extract_array::<f64>()?;
        let flat: Vec<f32> = arr.iter().map(|v| *v as f32).collect();
        Ok(flat)
    }
}

fn build_session(
    path: &Path,
    opt_level: ort::session::builder::GraphOptimizationLevel,
    intra_threads: usize,
    providers: &[String],
    dml_device_id: Option<i32>,
) -> anyhow::Result<ort::session::Session> {
    let env = ort_env();
    let mut b = ort::session::Session::builder(env).map_err(|e| anyhow::anyhow!("{e}"))?;
    b = oe(b.with_optimization_level(opt_level))?;
    b = oe(b.with_intra_threads(intra_threads))?;
    b = oe(b.with_inter_threads(1))?;
    // parallel execution
    b = oe(b.with_parallel_execution(false))?;

    let mut dispatch: Vec<ort::ep::ExecutionProviderDispatch> = Vec::new();
    for p in providers {
        match p.as_str() {
            // ort ships the DirectML EP only with the windows dep (Cargo.toml)
            #[cfg(windows)]
            "DmlExecutionProvider" => {
                let mut dml = ort::ep::DirectML::default();
                if let Some(id) = dml_device_id {
                    dml = dml.with_device_id(id);
                }
                dispatch.push(dml.into());
            }
            // ort ships the CUDA EP on Windows and Linux (target deps in Cargo.toml)
            #[cfg(any(windows, target_os = "linux"))]
            "CUDAExecutionProvider" => {
                let mut cuda = ort::ep::CUDA::default();
                if let Some(id) = dml_device_id {
                    cuda = cuda.with_device_id(id);
                }
                dispatch.push(cuda.into());
            }
            #[cfg(target_os = "linux")]
            "ROCMExecutionProvider" => {
                let mut rocm = ort::ep::ROCm::default();
                if let Some(id) = dml_device_id {
                    rocm = rocm.with_device_id(id);
                }
                dispatch.push(rocm.into());
            }
            #[cfg(all(target_os = "macos", feature = "coreml"))]
            "CoreMLExecutionProvider" => dispatch.push(ort::ep::CoreML::default().into()),
            "CPUExecutionProvider" => dispatch.push(ort::ep::CPU::default().into()),
                        _ => {} // provider not linked, ORT falls back to CPU
        }
    }
    if !dispatch.is_empty() {
        b = oe(b.with_execution_providers(dispatch))?;
    }
    oe(b.commit_from_file(path))
}

/// ort errors are not Send/Sync; wrapped via Display.
fn oe<T, E: std::fmt::Display>(r: Result<T, E>) -> anyhow::Result<T> {
    r.map_err(|e| anyhow::anyhow!("{e}"))
}

static ORT_ENV: std::sync::OnceLock<ort::environment::Environment> = std::sync::OnceLock::new();

pub fn ort_env() -> &'static ort::environment::Environment {
    ORT_ENV.get_or_init(|| ort::init().with_name("Tokenatra").build().expect("ort init"))
}

/// Loads the session with a fallback cascade:
/// EXTENDED + providers -> no device_id -> BASIC -> CPU.
fn load_session_inner(state: &AppState) -> anyhow::Result<Session> {
    let model_path = crate::paths::get_selected_model_path();
    if !model_path.exists() {
        anyhow::bail!("Модель не найдена: {}", model_path.display());
    }
    let cfg = state.provider_cfg.read().unwrap().clone();
    let (providers, dml_device_id) = match &cfg {
        Some(c) => (c.providers.clone(), c.dml_device_id),
        None => (vec!["CPUExecutionProvider".into()], None),
    };
    let cpu_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let intra = (cpu_count / 2).max(1);
    println!(
        "Модель: {} | Загрузка на {}…",
        model_path.file_name().map(|s| s.to_string_lossy()).unwrap_or_default(),
        state.device_name()
    );

    use ort::session::builder::GraphOptimizationLevel as G;
    let mut session = None;
    let mut cur_providers = providers.clone();
    let mut cur_dml = dml_device_id;

    // 1. EXTENDED + EP (+device_id)
    if let Ok(s) = try_build(&model_path, G::Level2, intra, &cur_providers, cur_dml) {
        session = Some(s);
    }
    // 2. no device_id (auto DML adapter selection failed)
    if session.is_none() && cur_dml.is_some() && cur_providers != ["CPUExecutionProvider".to_string()] {
        println!("  Адаптер #{:?} не запустился — пробуем дефолтный…", cur_dml);
        if let Ok(s) = try_build(&model_path, G::Level2, intra, &cur_providers, None) {
            session = Some(s);
            cur_dml = None;
            println!("  Дефолтный адаптер работает.");
        }
    }
    // 3. BASIC (known ORT LayerNorm-fusion bug)
    if session.is_none() {
        println!("  Оптимизация EXTENDED не загрузилась — откат на BASIC…");
        if let Ok(s) = try_build(&model_path, G::Level1, intra, &cur_providers, cur_dml) {
            session = Some(s);
        }
    }
    // 4. CPU
    let s = match session {
        Some(s) => s,
        None => {
            if cur_providers != ["CPUExecutionProvider".to_string()] {
                println!("  GPU-провайдер не запустился — откат на CPU…");
                state
                    .device_name
                    .write()
                    .unwrap()
                    .replace_range(.., "CPU (fallback)");
            }
            try_build(&model_path, G::Level1, intra, &["CPUExecutionProvider".to_string()], None)?
        }
    };
    cur_providers = vec!["CPUExecutionProvider".into()];
    let _ = cur_providers;

    let input_name = s.inputs()[0].name().to_string();
    let mut sess = Session { session: s, input_name };

    // warmup
    let t0 = Instant::now();
    let zeros = vec![0f32; 3 * 1024 * 1024];
    match sess.run(zeros) {
        Ok(_) => println!(" Прогрев завершён ({:.2} с).", t0.elapsed().as_secs_f32()),
        Err(e) => println!(" Прогрев не удался: {e}"),
    }
    println!(" Готово");
    Ok(sess)
}

fn try_build(
    path: &Path,
    level: ort::session::builder::GraphOptimizationLevel,
    intra: usize,
    providers: &[String],
    dml: Option<i32>,
) -> anyhow::Result<ort::session::Session> {
    build_session(path, level, intra, providers, dml)
}

/// Returns the live session, creating it if needed (under session_lock).
pub fn load_session(state: &Arc<AppState>) -> anyhow::Result<()> {
    let mut guard = state.session_lock.lock().unwrap();
    if guard.is_none() {
        *guard = Some(load_session_inner(state)?);
        crate::tray_control::notify_model_state(true);
        drop(guard);
        spawn_tune_child(state);
    }
    Ok(())
}

/// Unloads the ONNX session (tray idle). True if unloaded.
pub fn unload_model(state: &Arc<AppState>) -> bool {
    let mut guard = state.session_lock.lock().unwrap();
    if guard.is_none() {
        return false;
    }
    *guard = None;
    drop(guard);
    println!("[tray] Модель выгружена по простою в трее");
    crate::tray_control::notify_model_state(false);
    true
}

pub fn model_loaded(state: &Arc<AppState>) -> bool {
    state.session_lock.lock().unwrap().is_some()
}

/// Inference under session_lock (from session setup to run).
pub fn run_inference(state: &Arc<AppState>, tensor: Vec<f32>) -> anyhow::Result<Vec<f32>> {
    let mut guard = state.session_lock.lock().unwrap();
    if guard.is_none() {
        let t_infer = Instant::now();
        *guard = Some(load_session_inner(state)?);
        println!("  Загрузка модели: {:.2} с", t_infer.elapsed().as_secs_f32());
        crate::tray_control::notify_model_state(true);
    }
    let t0 = Instant::now();
    let sess = guard.as_mut().unwrap();
    let out = sess.run(tensor);
    println!("  Инференс: {:.2} с", t0.elapsed().as_secs_f32());
    out
}

/// Full remove_background pipeline.
pub fn remove_background(
    state: &Arc<AppState>,
    img: image::DynamicImage,
    edge_blur: f64,
) -> anyhow::Result<image::RgbaImage> {
    let img = crate::imaging::cap_process_size(img);
    let model_name = crate::paths::get_selected_model_path()
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let gentle = model_name.contains("ben");
    let imagenet = !gentle;

    let rgb = crate::imaging::flatten_to_rgb(&img);
    let (w, h) = rgb.dimensions();
    println!("  [RAM] старт обработки | размер входа: {w}x{h}");

    let tensor = crate::imaging::prepare_model_input(&rgb, imagenet);
    let raw = run_inference(state, tensor)?;
    let mask1024 = crate::imaging::mask_from_output(&raw)
        .ok_or_else(|| anyhow::anyhow!("маска пуста"))?;

    let mask_full = crate::imaging::refine_mask(
        &mask1024,
        edge_blur,
        10.0,
        245.0,
        1024,
        Some((w, h)),
        gentle,
    );

    let correction = crate::imaging::white_penalty_correction(&rgb, &mask_full);
    Ok(crate::imaging::composite_result(&rgb, &mask_full, &correction))
}

// ===== Device detection =====

fn run_cmd(cmd: &str, args: &[&str]) -> String {
    use std::process::{Command, Stdio};
    let mut c = Command::new(cmd);
    c.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c.output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

const VIRTUAL_ADAPTER_KEYWORDS: &[&str] = &[
    "parsec",
    "virtual",
    "microsoft",
    "basic",
    "remote",
    "indirect",
    "display only",
    "rdp",
    "teamviewer",
    "anydesk",
    "warp",
    "software renderer",
];

fn is_real_gpu(name: &str) -> bool {
    let low = name.to_lowercase();
    !VIRTUAL_ADAPTER_KEYWORDS.iter().any(|kw| low.contains(kw))
}

fn real_gpus() -> Vec<String> {
    let out = if cfg!(windows) {
        run_cmd(
            "powershell",
            &[
                "-Command",
                "Get-CimInstance Win32_VideoController | Select-Object -ExpandProperty Name",
            ],
        )
    } else {
        let nvidia = run_cmd("nvidia-smi", &["--query-gpu=name", "--format=csv,noheader"]);
        if !nvidia.trim().is_empty() {
            nvidia
        } else {
            run_cmd("rocm-smi", &["--showproductname"])
        }
    };
    out.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .filter(|l| is_real_gpu(l))
        .collect()
}

fn cpu_name() -> String {
    if cfg!(windows) {
        let n = run_cmd(
            "powershell",
            &["-Command", "(Get-CimInstance Win32_Processor | Select-Object -First 1).Name"],
        );
        return n.trim().to_string();
    }
    if cfg!(target_os = "macos") {
        return run_cmd("sysctl", &["-n", "machdep.cpu.brand_string"]).trim().to_string();
    }
    let grep = run_cmd("grep", &["-m1", "model name", "/proc/cpuinfo"]);
    grep.split(':').nth(1).unwrap_or_default().trim().to_string()
}

/// Classifies a DXGI adapter: discrete | integrated | virtual | None.
pub fn classify_adapter(name: &str) -> Option<&'static str> {
    let low = name.to_lowercase();
    let contains_any = |kws: &[&str]| kws.iter().any(|k| low.contains(k));
    if contains_any(&[
        "microsoft", "basic", "remote", "warp", "parsec", "virtual", "rdp", "indirect",
        "software", "teamviewer", "anydesk",
    ]) {
        return Some("virtual");
    }
    if contains_any(&["nvidia", "geforce", "rtx", "gtx", "quadro", "tesla"]) {
        return Some("discrete");
    }
    if low.contains("arc") {
        return Some("discrete");
    }
    if low.contains("intel") {
        return Some("integrated");
    }
    if low.contains("radeon") || low.contains("amd") {
        if low.ends_with("graphics") || low.contains("vega") || low.contains("ryzen") || low.contains("apu")
        {
            return Some("integrated");
        }
        return Some("discrete");
    }
    if low.contains("graphics") || low.contains("adreno") || low.contains("mali") {
        return Some("integrated");
    }
    None
}

/// Discrete card (lowest DXGI index) for DirectML on hybrid laptops.
pub fn detect_discrete_dml_adapter() -> (Option<i32>, Option<String>) {
    #[cfg(windows)]
    {
        match crate::win::enumerate_dxgi_adapters() {
            Some(list) => {
                let mut best: Option<(i32, String)> = None;
                for (idx, name) in &list {
                    if classify_adapter(name) == Some("discrete")
                        && best.as_ref().map(|b| *idx < b.0).unwrap_or(true) {
                            best = Some((*idx, name.clone()));
                        }
                }
                match best {
                    Some((i, n)) => (Some(i), Some(n)),
                    None => (None, None),
                }
            }
            None => (None, None),
        }
    }
    #[cfg(not(windows))]
    {
        (None, None)
    }
}

fn save_device_cache(providers: &[String], dml_device_id: Option<i32>, device_name: &str) {
    let provider_options = match dml_device_id {
        Some(id) => json!([{ "device_id": id }, {}]),
        None => serde_json::Value::Null,
    };
    let _ = crate::config::update(|cfg| {
        if let Some(obj) = cfg.as_object_mut() {
            obj.insert(
                "device_cache".into(),
                json!({
                    "providers": providers,
                    "provider_options": provider_options,
                    "device_name": device_name,
                }),
            );
        }
        Some(())
    });
}

fn read_device_cache() -> Option<(Vec<String>, Option<i32>, String)> {
    let cfg = crate::config::load().ok()?;
    let c = cfg.get("device_cache")?;
    let prov = c.get("providers")?.as_array()?;
    if prov.is_empty() {
        return None;
    }
    let names: Vec<String> = prov
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    if names.is_empty() || !names.iter().any(|p| p == "CPUExecutionProvider") {
        return None;
    }
    let avail = available_providers();
    if !names.iter().all(|p| avail.contains(&p.as_str())) {
        return None;
    }
    let mut dml_id = None;
    if let Some(opts) = c.get("provider_options").and_then(|v| v.as_array())
        && let Some(first) = opts.first().and_then(|o| o.get("device_id")).and_then(|v| v.as_i64())
        {
            dml_id = Some(first as i32);
        }
    // manual gpu_device_id in config.json must match the cache
    if let Some(dev) = crate::config::gpu_device_id()
        && dml_id != Some(dev as i32) {
            return None;
        }
    let name = c.get("device_name")?.as_str()?.to_string();
    if name.is_empty() {
        return None;
    }
    Some((names, dml_id, name))
}

/// Detects the device and caches the result.
pub fn detect_device(state: &AppState) {
    if let Some((providers, dml_id, name)) = read_device_cache() {
        println!("  Устройство (кеш): {name}");
        *state.device_name.write().unwrap() = name;
        *state.provider_cfg.write().unwrap() = Some(ProviderConfig { providers, dml_device_id: dml_id });
        return;
    }

    let avail = available_providers();

    // macOS: CoreML resolves immediately
    if cfg!(target_os = "macos") && avail.contains(&"CoreMLExecutionProvider") {
        *state.device_name.write().unwrap() = "CoreML".into();
        *state.provider_cfg.write().unwrap() = Some(ProviderConfig {
            providers: vec!["CoreMLExecutionProvider".into(), "CPUExecutionProvider".into()],
            dml_device_id: None,
        });
        save_device_cache(
            &["CoreMLExecutionProvider".into(), "CPUExecutionProvider".into()],
            None,
            "CoreML",
        );
        return;
    }

    let gpus = real_gpus();
    let gpu_name = gpus.first().cloned();
    let nvidia = gpus
        .iter()
        .find(|g| g.to_lowercase().contains("nvidia"))
        .cloned();
    let has_cuda = avail.contains(&"CUDAExecutionProvider");
    let has_rocm = avail.contains(&"ROCMExecutionProvider");
    let has_dml = avail.contains(&"DmlExecutionProvider");

    if let (Some(n), true) = (&nvidia, has_cuda) {
        let name = format!("CUDA ({n})");
        println!("  Устройство: {name}");
        *state.device_name.write().unwrap() = name.clone();
        *state.provider_cfg.write().unwrap() = Some(ProviderConfig {
            providers: vec!["CUDAExecutionProvider".into(), "CPUExecutionProvider".into()],
            dml_device_id: None,
        });
        save_device_cache(
            &["CUDAExecutionProvider".into(), "CPUExecutionProvider".into()],
            None,
            &name,
        );
        return;
    }

    // ROCm is AMD-only; Intel GPU falls back to CPU
    let is_amd_gpu = gpu_name
        .as_ref()
        .map(|g| {
            let l = g.to_lowercase();
            l.contains("radeon") || l.contains("amd")
        })
        .unwrap_or(false);
    if let (Some(g), true) = (&gpu_name, has_rocm && is_amd_gpu) {
        let name = format!("ROCm ({g})");
        *state.device_name.write().unwrap() = name.clone();
        *state.provider_cfg.write().unwrap() = Some(ProviderConfig {
            providers: vec!["ROCMExecutionProvider".into(), "CPUExecutionProvider".into()],
            dml_device_id: None,
        });
        save_device_cache(
            &["ROCMExecutionProvider".into(), "CPUExecutionProvider".into()],
            None,
            &name,
        );
        return;
    }

    if let Some(g) = gpu_name.filter(|_| has_dml) {
        let mut dml_id = crate::config::gpu_device_id().map(|v| v as i32);
        let mut shown = g.clone();
        if dml_id.is_none() {
            let (id, dname) = detect_discrete_dml_adapter();
            if let (Some(i), Some(n)) = (id, dname) {
                dml_id = Some(i);
                shown = n;
                println!("  Дискретная видеокарта: {shown} (адаптер #{i})");
            }
        }
        let name = format!("DirectML ({shown})");
        *state.device_name.write().unwrap() = name.clone();
        *state.provider_cfg.write().unwrap() = Some(ProviderConfig {
            providers: vec!["DmlExecutionProvider".into(), "CPUExecutionProvider".into()],
            dml_device_id: dml_id,
        });
        save_device_cache(
            &["DmlExecutionProvider".into(), "CPUExecutionProvider".into()],
            dml_id,
            &name,
        );
        return;
    }

    let cn = cpu_name();
    let name = if cn.is_empty() { "CPU".into() } else { cn };
    *state.device_name.write().unwrap() = name.clone();
    *state.provider_cfg.write().unwrap() = Some(ProviderConfig {
        providers: vec!["CPUExecutionProvider".into()],
        dml_device_id: None,
    });
    save_device_cache(&["CPUExecutionProvider".into()], None, &name);
}

/// Background DirectML adapter tuning in a child process.
fn spawn_tune_child(state: &Arc<AppState>) {
    let cfg = state.provider_cfg.read().unwrap().clone();
    let Some(c) = cfg else { return };
    if !c.providers.iter().any(|p| p == "DmlExecutionProvider") || c.dml_device_id.is_some() {
        return;
    }
    if crate::config::gpu_device_id().is_some() {
        return;
    }
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(5));
        let exe = std::env::current_exe().unwrap_or_default();
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--tune-gpu");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS
            cmd.creation_flags(0x0800_0000 | 0x0000_4000);
        }
        match cmd.spawn() {
            Ok(_) => println!(" Запущен фоновый подбор адаптера DirectML (отдельный процесс)…"),
            Err(e) => println!(" Не удалось запустить подбор адаптера: {e}"),
        }
    });
}

/// CLI --tune-gpu: picks the fastest DirectML adapter.
pub fn cli_tune_gpu() {
    if !available_providers().contains(&"DmlExecutionProvider") {
        return;
    }
    let _ = ort_env();
    let model_path: PathBuf = crate::paths::get_selected_model_path();
    if !model_path.exists() {
        return;
    }
    let bench = |device_id: Option<i32>| -> anyhow::Result<f64> {
        use ort::session::builder::GraphOptimizationLevel as G;
        let s = build_session(&model_path, G::Level1, 2, &["DmlExecutionProvider".into(), "CPUExecutionProvider".into()], device_id)?;
        let name = s.inputs()[0].name().to_string();
        let mut sess = Session { session: s, input_name: name };
        let t0 = Instant::now();
        sess.run(vec![0f32; 3 * 1024 * 1024])?;
        Ok(t0.elapsed().as_secs_f64())
    };
    let default_t = match bench(None) {
        Ok(t) => {
            println!("  [tune] дефолтный адаптер: {t:.2} с");
            t
        }
        Err(e) => {
            println!("  [tune] ошибка: {e}");
            return;
        }
    };
    // DXGI есть только на Windows; иначе тюнинг идёт по двум device id
    #[cfg(windows)]
    let adapter_count = crate::win::enumerate_dxgi_adapters().map(|a| a.len()).unwrap_or(2);
    #[cfg(not(windows))]
    let adapter_count = 2;
    let limit = adapter_count.clamp(2, 4);
    let mut best: Option<(i32, f64)> = None;
    for did in 0..limit {
        let did = did as i32;
        match bench(Some(did)) {
            Ok(t) => {
                println!("  [tune] адаптер #{did}: {t:.2} с");
                if best.as_ref().map(|b| t < b.1).unwrap_or(true) {
                    best = Some((did, t));
                }
            }
            Err(e) => {
                println!("  [tune] адаптер #{did}: не доступен ({e})");
                break;
            }
        }
    }
    let Some((best_id, best_time)) = best else { return };
    if best_time >= default_t * 0.85 {
        println!("  [tune] текущий адаптер оптимален ({default_t:.2} с) — оставляем");
        return;
    }
    let _ = crate::config::set_key("gpu_device_id", json!(best_id));
    println!("  [tune] выбран адаптер #{best_id} ({best_time:.2} с), сохранено в config.json");
}
