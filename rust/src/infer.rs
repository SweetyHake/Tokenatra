use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde_json::json;

use crate::state::{AppState, ProviderConfig};

/// Providers linked into this binary.
pub fn available_providers() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        #[cfg(feature = "cuda")]
        {
            &["CUDAExecutionProvider", "DmlExecutionProvider", "CPUExecutionProvider"]
        }
        #[cfg(not(feature = "cuda"))]
        {
            &["DmlExecutionProvider", "CPUExecutionProvider"]
        }
    }
    #[cfg(target_os = "macos")]
    {
        &["CoreMLExecutionProvider", "CPUExecutionProvider"]
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        #[cfg(all(feature = "cuda", feature = "rocm"))]
        {
            &["CUDAExecutionProvider", "ROCMExecutionProvider", "CPUExecutionProvider"]
        }
        #[cfg(all(feature = "cuda", not(feature = "rocm")))]
        {
            &["CUDAExecutionProvider", "CPUExecutionProvider"]
        }
        #[cfg(all(not(feature = "cuda"), feature = "rocm"))]
        {
            &["ROCMExecutionProvider", "CPUExecutionProvider"]
        }
        #[cfg(not(any(feature = "cuda", feature = "rocm")))]
        {
            &["CPUExecutionProvider"]
        }
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
        // models output f32, f16 (BEN2) or f64
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

#[cfg_attr(not(any(windows, feature = "cuda", feature = "rocm")), allow(unused_variables))]
#[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
fn build_session(
    path: &Path,
    opt_level: ort::session::builder::GraphOptimizationLevel,
    intra_threads: usize,
    providers: &[String],
    device_id: Option<i32>,
    coreml_mlprogram: bool,
) -> anyhow::Result<ort::session::Session> {
    let env = ort_env();
    let mut b = ort::session::Session::builder(env).map_err(|e| anyhow::anyhow!("{e}"))?;
    b = oe(b.with_optimization_level(opt_level))?;
    b = oe(b.with_intra_threads(intra_threads))?;
    b = oe(b.with_inter_threads(1))?;
    b = oe(b.with_parallel_execution(false))?;
    // DirectML requires memory pattern optimization to be disabled
    if providers.iter().any(|p| p == "DmlExecutionProvider") {
        b = oe(b.with_memory_pattern(false))?;
    }

    let mut dispatch: Vec<ort::ep::ExecutionProviderDispatch> = Vec::new();
    for p in providers {
        match p.as_str() {
            // ort ships the DirectML EP only with the windows dep (Cargo.toml).
            // `.error_on_failure()` prevents ORT from silently swallowing GPU registration
            // errors and running on CPU while reporting a GPU device name.
            #[cfg(windows)]
            "DmlExecutionProvider" => {
                let mut dml = ort::ep::DirectML::default().with_performance_preference(
                    ort::ep::directml::PerformancePreference::HighPerformance,
                );
                if let Some(id) = device_id {
                    dml = dml.with_device_id(id);
                }
                dispatch.push(dml.build().error_on_failure());
            }
            #[cfg(feature = "cuda")]
            "CUDAExecutionProvider" => {
                let mut cuda = ort::ep::CUDA::default();
                if let Some(id) = device_id {
                    cuda = cuda.with_device_id(id);
                }
                dispatch.push(cuda.build().error_on_failure());
            }
            #[cfg(feature = "rocm")]
            "ROCMExecutionProvider" => {
                let mut rocm = ort::ep::ROCm::default();
                if let Some(id) = device_id {
                    rocm = rocm.with_device_id(id);
                }
                dispatch.push(rocm.build().error_on_failure());
            }
            #[cfg(target_os = "macos")]
            "CoreMLExecutionProvider" => {
                let mut coreml = ort::ep::CoreML::default()
                    .with_compute_units(ort::ep::coreml::ComputeUnits::All);
                if coreml_mlprogram {
                    coreml = coreml.with_model_format(ort::ep::coreml::ModelFormat::MLProgram);
                }
                dispatch.push(coreml.build().error_on_failure());
            }
            "CPUExecutionProvider" => dispatch.push(ort::ep::CPU::default().build()),
            _ => {}
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

fn try_build_and_warmup(
    path: &Path,
    level: ort::session::builder::GraphOptimizationLevel,
    intra: usize,
    providers: &[String],
    device_id: Option<i32>,
    coreml_mlprogram: bool,
) -> anyhow::Result<Session> {
    let s = build_session(path, level, intra, providers, device_id, coreml_mlprogram)?;
    let input_name = s.inputs()[0].name().to_string();
    let mut sess = Session {
        session: s,
        input_name,
    };
    let t0 = Instant::now();
    let zeros = vec![0f32; 3 * 1024 * 1024];
    sess.run(zeros)?;
    println!(" Прогрев завершён ({:.2} с).", t0.elapsed().as_secs_f32());
    Ok(sess)
}

/// Loads the session with a verified fallback cascade:
/// EXTENDED + GPU (+device_id) -> default GPU adapter -> BASIC + GPU -> (DML fallback if CUDA failed) -> CPU.
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
    let cpu_only_list = ["CPUExecutionProvider".to_string()];
    let is_cpu_only = providers == cpu_only_list;

    let cpu_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let intra = (cpu_count / 2).max(1);
    println!(
        "Модель: {} | Загрузка на {}…",
        model_path
            .file_name()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default(),
        state.device_name()
    );

    use ort::session::builder::GraphOptimizationLevel as G;
    let mut session = None;
    let mut cur_dml = dml_device_id;

    if !is_cpu_only {
        // 1. EXTENDED + GPU EP (+device_id)
        match try_build_and_warmup(&model_path, G::Level2, intra, &providers, cur_dml, true) {
            Ok(s) => session = Some(s),
            Err(e) => println!("  Попытка EXTENDED не удалась: {e}"),
        }

        // 2. Default adapter (no explicit device_id) if a specific device_id failed
        if session.is_none() && cur_dml.is_some() {
            println!(
                "  Адаптер #{:?} не запустился — пробуем дефолтный…",
                cur_dml
            );
            match try_build_and_warmup(&model_path, G::Level2, intra, &providers, None, true) {
                Ok(s) => {
                    session = Some(s);
                    cur_dml = None;
                    let _ = crate::config::remove_key("gpu_device_id");
                    if let Some(cfg_mut) = state.provider_cfg.write().unwrap().as_mut() {
                        cfg_mut.dml_device_id = None;
                    }
                    println!("  Дефолтный адаптер работает.");
                }
                Err(e) => println!("  Дефолтный адаптер (EXTENDED) не запустился: {e}"),
            }
        }

        // 3. BASIC graph optimization (known ORT LayerNorm-fusion fallback; also NeuralNetwork on old macOS)
        if session.is_none() {
            println!("  Оптимизация EXTENDED не загрузилась — откат на BASIC…");
            if let Ok(s) =
                try_build_and_warmup(&model_path, G::Level1, intra, &providers, cur_dml, false)
            {
                session = Some(s);
            } else if cur_dml.is_some()
                && let Ok(s) =
                    try_build_and_warmup(&model_path, G::Level1, intra, &providers, None, false)
            {
                session = Some(s);
                let _ = crate::config::remove_key("gpu_device_id");
                if let Some(cfg_mut) = state.provider_cfg.write().unwrap().as_mut() {
                    cfg_mut.dml_device_id = None;
                }
            }
        }

        // 3b. Windows custom `--features cuda` build without local CUDA/cuDNN SDK: fall back to DirectML before CPU
        if session.is_none()
            && providers.iter().any(|p| p == "CUDAExecutionProvider")
            && available_providers().contains(&"DmlExecutionProvider")
        {
            println!("  CUDA не запустилась — переключение на DirectML…");
            let dml_provs: Vec<String> =
                vec!["DmlExecutionProvider".into(), "CPUExecutionProvider".into()];
            let (dml_id, dml_label) = best_windows_dml_target();
            if let Ok(s) =
                try_build_and_warmup(&model_path, G::Level2, intra, &dml_provs, dml_id, false)
                    .or_else(|_| {
                        try_build_and_warmup(
                            &model_path,
                            G::Level1,
                            intra,
                            &dml_provs,
                            None,
                            false,
                        )
                    })
            {
                let new_name = format!(
                    "DirectML ({})",
                    dml_label.unwrap_or_else(|| "GPU".into())
                );
                *state.device_name.write().unwrap() = new_name.clone();
                *state.provider_cfg.write().unwrap() = Some(ProviderConfig {
                    providers: dml_provs.clone(),
                    dml_device_id: dml_id,
                });
                save_device_cache(&dml_provs, dml_id, &new_name);
                session = Some(s);
            }
        }
    }

    // 4. CPU fallback
    let sess = match session {
        Some(s) => s,
        None => {
            if !is_cpu_only {
                println!("  GPU-провайдер не запустился — откат на CPU…");
                state
                    .device_name
                    .write()
                    .unwrap()
                    .replace_range(.., "CPU (fallback)");
            }
            try_build_and_warmup(&model_path, G::Level1, intra, &cpu_only_list, None, false)?
        }
    };

    println!(" Готово");
    Ok(sess)
}

/// Returns the live session, creating it if needed (under session_lock).
pub fn load_session(state: &Arc<AppState>) -> anyhow::Result<()> {
    let mut guard = state.session_lock.lock().unwrap();
    if guard.is_none() {
        *guard = Some(load_session_inner(state)?);
        state.set_model_loaded(true);
        drop(guard);
        spawn_tune_child(state);
    }
    Ok(())
}

/// Unloads the ONNX session. True if unloaded.
pub fn unload_model(state: &Arc<AppState>) -> bool {
    let mut guard = state.session_lock.lock().unwrap();
    if guard.is_none() {
        state.set_model_loaded(false);
        return false;
    }
    *guard = None;
    state.set_model_loaded(false);
    drop(guard);
    println!("[tray] Модель выгружена по простою");
    true
}

pub fn model_loaded(state: &Arc<AppState>) -> bool {
    state.is_model_loaded()
}

/// Inference under session_lock (from session setup to run).
pub fn run_inference(state: &Arc<AppState>, tensor: Vec<f32>) -> anyhow::Result<Vec<f32>> {
    let mut guard = state.session_lock.lock().unwrap();
    let mut freshly_loaded = false;
    if guard.is_none() {
        let t_infer = Instant::now();
        *guard = Some(load_session_inner(state)?);
        println!("  Загрузка модели: {:.2} с", t_infer.elapsed().as_secs_f32());
        state.set_model_loaded(true);
        freshly_loaded = true;
    }
    let t0 = Instant::now();
    let sess = guard.as_mut().unwrap();
    let out = sess.run(tensor);
    println!("  Инференс: {:.2} с", t0.elapsed().as_secs_f32());
    drop(guard);
    if freshly_loaded {
        spawn_tune_child(state);
    }
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
    let gentle = !model_name.contains("birefnet") && !model_name.contains("rmbg");
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

#[cfg(not(windows))]
fn run_cmd(cmd: &str, args: &[&str]) -> String {
    use std::process::{Command, Stdio};
    let mut c = Command::new(cmd);
    c.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    c.output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

#[cfg_attr(windows, allow(dead_code))]
const VIRTUAL_ADAPTER_KEYWORDS: &[&str] = &[
    "parsec",
    "virtual",
    "microsoft basic",
    "microsoft remote",
    "hyper-v",
    "basic render",
    "basic display",
    "remote",
    "indirect",
    "display only",
    "rdp",
    "teamviewer",
    "anydesk",
    "warp",
    "software renderer",
    "vmware",
    "vbox",
    "qxl",
    "citrix",
    "spacedesk",
];

#[cfg_attr(windows, allow(dead_code))]
fn is_real_gpu(name: &str) -> bool {
    let low = name.to_lowercase();
    !VIRTUAL_ADAPTER_KEYWORDS.iter().any(|kw| low.contains(kw))
}

#[cfg(not(windows))]
fn real_gpus_unix() -> Vec<String> {
    let nvidia = run_cmd("nvidia-smi", &["--query-gpu=name", "--format=csv,noheader"]);
    let out = if !nvidia.trim().is_empty() {
        nvidia
    } else {
        run_cmd("rocm-smi", &["--showproductname"])
    };
    out.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .filter(|l| is_real_gpu(l))
        .collect()
}

fn cpu_name() -> String {
    #[cfg(windows)]
    {
        crate::win::cpu_name_from_registry().unwrap_or_default()
    }
    #[cfg(target_os = "macos")]
    {
        run_cmd("sysctl", &["-n", "machdep.cpu.brand_string"])
            .trim()
            .to_string()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let grep = run_cmd("grep", &["-m1", "model name", "/proc/cpuinfo"]);
        grep.split(':').nth(1).unwrap_or_default().trim().to_string()
    }
}

/// Detects mobile Radeon iGPU model tokens like `680m`, `780m`, `890m` (3 digits + `m`, without `rx`).
fn has_radeon_igpu_model(low: &str) -> bool {
    if low.contains("rx ") || low.contains("pro ") {
        return false;
    }
    for tok in low.split(|c: char| !c.is_ascii_alphanumeric()) {
        if tok.len() == 4
            && tok.ends_with('m')
            && tok[..3].chars().all(|c| c.is_ascii_digit())
        {
            return true;
        }
    }
    false
}

/// True when the GPU is integrated rather than a dedicated card.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn is_integrated_gpu(name: &str, vendor_id: u32, dedicated_vram: u64) -> bool {
    const THREE_GIB: u64 = 3 * 1024 * 1024 * 1024;
    // Any adapter with >= 3 GiB dedicated VRAM is a discrete GPU (or equivalent high-perf carveout)
    if dedicated_vram >= THREE_GIB {
        return false;
    }

    let low = name.to_lowercase();
    let contains_any = |kws: &[&str]| kws.iter().any(|k| low.contains(k));

    // NVIDIA GPUs are discrete
    if vendor_id == 0x10DE
        || contains_any(&["nvidia", "geforce", "rtx", "gtx", "quadro", "tesla"])
    {
        return false;
    }

    // Intel: discrete Arc A/B/Pro series vs Core Ultra Arc/Iris/UHD iGPUs
    if vendor_id == 0x8086 || low.contains("intel") {
        if dedicated_vram > 0 {
            // Known VRAM < 3 GiB -> integrated Intel GPU (including Core Ultra "Intel Arc Graphics" / 140V)
            return true;
        }
        if low.contains("arc")
            && contains_any(&[
                "a310", "a380", "a580", "a750", "a770", "b570", "b580", "a350m", "a370m",
                "a550m", "a730m", "a770m", "arc pro",
            ])
        {
            return false;
        }
        return true;
    }

    // AMD: Radeon RX / Pro discrete vs Radeon Graphics / Vega / 780M / 890M iGPUs
    if vendor_id == 0x1002 || low.contains("radeon") || low.contains("amd") {
        if low.ends_with("graphics")
            || low.contains("vega")
            || low.contains("ryzen")
            || low.contains("apu")
            || has_radeon_igpu_model(&low)
        {
            return true;
        }
        if dedicated_vram > 0 && dedicated_vram < 1536 * 1024 * 1024 {
            return true;
        }
        return false;
    }

    if contains_any(&["adreno", "mali", "uhd", "iris"]) || low.ends_with("graphics") {
        return true;
    }
    false
}

/// Classifies a DXGI adapter name: discrete | integrated | virtual | None.
#[allow(dead_code)]
pub fn classify_adapter(name: &str) -> Option<&'static str> {
    if !is_real_gpu(name) || name.to_lowercase().contains("microsoft basic") {
        return Some("virtual");
    }
    if is_integrated_gpu(name, 0, 0) {
        Some("integrated")
    } else {
        Some("discrete")
    }
}

#[cfg(windows)]
fn pick_best_dxgi_adapter(
    real_adapters: &[crate::win::DxgiAdapterInfo],
) -> Option<&crate::win::DxgiAdapterInfo> {
    real_adapters.iter().max_by_key(|a| {
        let discrete = !is_integrated_gpu(&a.name, a.vendor_id, a.dedicated_vram);
        let vram_bucket = a.dedicated_vram / (256 * 1024 * 1024);
        (discrete, a.is_high_perf_first, vram_bucket, -a.index)
    })
}

fn best_windows_dml_target() -> (Option<i32>, Option<String>) {
    #[cfg(windows)]
    {
        let Some(all) = crate::win::enumerate_dxgi_adapters_detailed() else {
            return (None, None);
        };
        let real: Vec<_> = all.into_iter().filter(|a| !a.is_software).collect();
        if real.is_empty() {
            return (None, None);
        }
        if let Some(manual) = crate::config::gpu_device_id() {
            let manual_i32 = manual as i32;
            if let Some(found) = real.iter().find(|a| a.index == manual_i32) {
                return (Some(found.index), Some(found.name.clone()));
            }
            // Stale or software adapter index in config.json (e.g. old --tune-gpu bug): clean it up
            println!(
                "  Игнорируем неактуальный gpu_device_id={manual_i32} в config.json и выбираем реальный GPU"
            );
            let _ = crate::config::remove_key("gpu_device_id");
        }
        if let Some(best) = pick_best_dxgi_adapter(&real) {
            return (Some(best.index), Some(best.name.clone()));
        }
        (None, None)
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

#[cfg(not(any(windows, target_os = "macos")))]
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
    // Never lock onto a cached CPU-only entry; re-probe for hardware GPUs on startup
    if names.is_empty()
        || !names.iter().any(|p| p == "CPUExecutionProvider")
        || !names.iter().any(|p| p != "CPUExecutionProvider")
    {
        return None;
    }
    let avail = available_providers();
    if !names.iter().all(|p| avail.contains(&p.as_str())) {
        return None;
    }
    let mut dml_id = None;
    if let Some(opts) = c.get("provider_options").and_then(|v| v.as_array())
        && let Some(first) = opts
            .first()
            .and_then(|o| o.get("device_id"))
            .and_then(|v| v.as_i64())
    {
        dml_id = Some(first as i32);
    }
    if let Some(dev) = crate::config::gpu_device_id()
        && dml_id != Some(dev as i32)
    {
        return None;
    }
    let name = c.get("device_name")?.as_str()?.to_string();
    if name.is_empty() {
        return None;
    }
    Some((names, dml_id, name))
}

/// Detects the device and updates AppState + config cache.
pub fn detect_device(state: &AppState) {
    let avail = available_providers();

    // Windows: fast native DXGI enumeration (~0.1ms, zero child processes).
    // Always probes live DXGI adapters so stale caches or GPU swaps self-heal immediately.
    #[cfg(windows)]
    {
        let all = crate::win::enumerate_dxgi_adapters_detailed().unwrap_or_default();
        let real: Vec<_> = all.into_iter().filter(|a| !a.is_software).collect();

        if avail.contains(&"CUDAExecutionProvider")
            && let Some(nv) = real
                .iter()
                .find(|a| a.vendor_id == 0x10DE || a.name.to_lowercase().contains("nvidia"))
        {
            let name = format!("CUDA ({})", nv.name);
            let provs: Vec<String> =
                vec!["CUDAExecutionProvider".into(), "CPUExecutionProvider".into()];
            println!("  Устройство: {name}");
            *state.device_name.write().unwrap() = name.clone();
            *state.provider_cfg.write().unwrap() = Some(ProviderConfig {
                providers: provs.clone(),
                dml_device_id: None,
            });
            save_device_cache(&provs, None, &name);
            return;
        }

        if avail.contains(&"DmlExecutionProvider")
            && let (Some(dml_id), Some(shown)) = best_windows_dml_target()
        {
            let name = format!("DirectML ({shown})");
            let provs: Vec<String> =
                vec!["DmlExecutionProvider".into(), "CPUExecutionProvider".into()];
            println!("  Устройство: {name} (DXGI адаптер #{dml_id})");
            *state.device_name.write().unwrap() = name.clone();
            *state.provider_cfg.write().unwrap() = Some(ProviderConfig {
                providers: provs.clone(),
                dml_device_id: Some(dml_id),
            });
            save_device_cache(&provs, Some(dml_id), &name);
            return;
        }

        let cn = cpu_name();
        let name = if cn.is_empty() { "CPU".into() } else { cn };
        let _ = crate::config::remove_key("device_cache");
        *state.device_name.write().unwrap() = name;
        *state.provider_cfg.write().unwrap() = Some(ProviderConfig {
            providers: vec!["CPUExecutionProvider".into()],
            dml_device_id: None,
        });
    }

    // macOS: CoreML resolves immediately
    #[cfg(target_os = "macos")]
    {
        if avail.contains(&"CoreMLExecutionProvider") {
            let provs: Vec<String> = vec![
                "CoreMLExecutionProvider".into(),
                "CPUExecutionProvider".into(),
            ];
            *state.device_name.write().unwrap() = "CoreML".into();
            *state.provider_cfg.write().unwrap() = Some(ProviderConfig {
                providers: provs.clone(),
                dml_device_id: None,
            });
            save_device_cache(&provs, None, "CoreML");
            return;
        }
        let cn = cpu_name();
        let name = if cn.is_empty() { "CPU".into() } else { cn };
        *state.device_name.write().unwrap() = name;
        *state.provider_cfg.write().unwrap() = Some(ProviderConfig {
            providers: vec!["CPUExecutionProvider".into()],
            dml_device_id: None,
        });
    }

    // Linux / other Unix
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        if let Some((providers, dml_id, name)) = read_device_cache() {
            println!("  Устройство (кеш): {name}");
            *state.device_name.write().unwrap() = name;
            *state.provider_cfg.write().unwrap() = Some(ProviderConfig {
                providers,
                dml_device_id: dml_id,
            });
            return;
        }

        let gpus = real_gpus_unix();
        let gpu_name = gpus.first().cloned();
        let nvidia = gpus
            .iter()
            .find(|g| g.to_lowercase().contains("nvidia"))
            .cloned();
        let has_cuda = avail.contains(&"CUDAExecutionProvider");
        let has_rocm = avail.contains(&"ROCMExecutionProvider");

        if let (Some(n), true) = (&nvidia, has_cuda) {
            let name = format!("CUDA ({n})");
            let provs: Vec<String> =
                vec!["CUDAExecutionProvider".into(), "CPUExecutionProvider".into()];
            println!("  Устройство: {name}");
            *state.device_name.write().unwrap() = name.clone();
            *state.provider_cfg.write().unwrap() = Some(ProviderConfig {
                providers: provs.clone(),
                dml_device_id: None,
            });
            save_device_cache(&provs, None, &name);
            return;
        }

        let is_amd_gpu = gpu_name
            .as_ref()
            .map(|g| {
                let l = g.to_lowercase();
                l.contains("radeon") || l.contains("amd")
            })
            .unwrap_or(false);
        if let (Some(g), true) = (&gpu_name, has_rocm && is_amd_gpu) {
            let name = format!("ROCm ({g})");
            let provs: Vec<String> =
                vec!["ROCMExecutionProvider".into(), "CPUExecutionProvider".into()];
            *state.device_name.write().unwrap() = name.clone();
            *state.provider_cfg.write().unwrap() = Some(ProviderConfig {
                providers: provs.clone(),
                dml_device_id: None,
            });
            save_device_cache(&provs, None, &name);
            return;
        }

        let cn = cpu_name();
        let name = if cn.is_empty() { "CPU".into() } else { cn };
        let _ = crate::config::remove_key("device_cache");
        *state.device_name.write().unwrap() = name;
        *state.provider_cfg.write().unwrap() = Some(ProviderConfig {
            providers: vec!["CPUExecutionProvider".into()],
            dml_device_id: None,
        });
    }
}

/// Background DirectML adapter tuning in a child process (only when >=2 real GPUs and no adapter chosen).
#[cfg_attr(not(windows), allow(dead_code))]
fn spawn_tune_child(state: &Arc<AppState>) {
    let cfg = state.provider_cfg.read().unwrap().clone();
    let Some(c) = cfg else { return };
    if !c.providers.iter().any(|p| p == "DmlExecutionProvider") || c.dml_device_id.is_some() {
        return;
    }
    if crate::config::gpu_device_id().is_some() {
        return;
    }
    #[cfg(windows)]
    {
        let real_count = crate::win::enumerate_dxgi_adapters_detailed()
            .map(|list| list.iter().filter(|a| !a.is_software).count())
            .unwrap_or(0);
        if real_count < 2 {
            return;
        }
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

/// CLI --tune-gpu: benchmarks real hardware DirectML adapters (with shader warmup) and saves the fastest one.
pub fn cli_tune_gpu() {
    if !available_providers().contains(&"DmlExecutionProvider") {
        return;
    }
    #[cfg(windows)]
    let real_adapters: Vec<crate::win::DxgiAdapterInfo> =
        crate::win::enumerate_dxgi_adapters_detailed()
            .unwrap_or_default()
            .into_iter()
            .filter(|a| !a.is_software)
            .collect();
    #[cfg(windows)]
    if real_adapters.len() < 2 {
        return;
    }

    let _ = ort_env();
    let model_path: PathBuf = crate::paths::get_selected_model_path();
    if !model_path.exists() {
        return;
    }
    let bench = |device_id: Option<i32>| -> anyhow::Result<f64> {
        use ort::session::builder::GraphOptimizationLevel as G;
        let s = build_session(
            &model_path,
            G::Level1,
            2,
            &["DmlExecutionProvider".into(), "CPUExecutionProvider".into()],
            device_id,
            false,
        )?;
        let name = s.inputs()[0].name().to_string();
        let mut sess = Session {
            session: s,
            input_name: name,
        };
        // Warmup run compiles D3D12 shaders; measure the second run for true inference speed
        sess.run(vec![0f32; 3 * 1024 * 1024])?;
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

    let mut best: Option<(i32, f64)> = None;
    #[cfg(windows)]
    for adapter in &real_adapters {
        let did = adapter.index;
        match bench(Some(did)) {
            Ok(t) => {
                println!("  [tune] адаптер #{did} ({}): {t:.2} с", adapter.name);
                if best.as_ref().map(|b| t < b.1).unwrap_or(true) {
                    best = Some((did, t));
                }
            }
            Err(e) => {
                println!("  [tune] адаптер #{did} ({}): не доступен ({e})", adapter.name);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integrated_vs_discrete_classification() {
        const MB: u64 = 1024 * 1024;
        const GB: u64 = 1024 * MB;

        // NVIDIA cards are always discrete
        assert!(!is_integrated_gpu("NVIDIA GeForce RTX 4070", 0x10DE, 12 * GB));
        assert!(!is_integrated_gpu("NVIDIA GeForce GTX 1650", 0x10DE, 4 * GB));

        // AMD discrete vs APU / Ryzen iGPU
        assert!(!is_integrated_gpu("AMD Radeon RX 7900 GRE", 0x1002, 16 * GB));
        assert!(!is_integrated_gpu("AMD Radeon RX 7600M XT", 0x1002, 8 * GB));
        assert!(is_integrated_gpu("AMD Radeon 780M", 0x1002, 512 * MB));
        assert!(is_integrated_gpu("AMD Radeon 890M", 0x1002, 1024 * MB));
        assert!(is_integrated_gpu("AMD Radeon(TM) Graphics", 0x1002, 512 * MB));

        // Intel discrete Arc vs Core Ultra Arc/Iris/UHD iGPU
        assert!(!is_integrated_gpu("Intel(R) Arc(TM) B580 Graphics", 0x8086, 12 * GB));
        assert!(!is_integrated_gpu("Intel(R) Arc(TM) A770 Graphics", 0x8086, 16 * GB));
        assert!(is_integrated_gpu("Intel(R) Arc(TM) 140V GPU", 0x8086, 128 * MB));
        assert!(is_integrated_gpu("Intel(R) Arc(TM) Graphics", 0x8086, 128 * MB));
        assert!(is_integrated_gpu("Intel(R) Iris(R) Xe Graphics", 0x8086, 128 * MB));
    }

    #[cfg(windows)]
    #[test]
    fn hybrid_laptop_adapter_selection() {
        use crate::win::DxgiAdapterInfo;
        const MB: u64 = 1024 * 1024;
        const GB: u64 = 1024 * MB;

        // Ryzen 7840HS (780M at index 0) + RTX 4060 Laptop (at index 1)
        let adapters = vec![
            DxgiAdapterInfo {
                index: 0,
                name: "AMD Radeon 780M".into(),
                vendor_id: 0x1002,
                dedicated_vram: 512 * MB,
                is_software: false,
                is_high_perf_first: false,
            },
            DxgiAdapterInfo {
                index: 1,
                name: "NVIDIA GeForce RTX 4060 Laptop GPU".into(),
                vendor_id: 0x10DE,
                dedicated_vram: 8 * GB,
                is_software: false,
                is_high_perf_first: true,
            },
        ];
        let best = pick_best_dxgi_adapter(&adapters).unwrap();
        assert_eq!(best.index, 1);

        // Single integrated GPU laptop -> picks that integrated GPU (index 0)
        let single_igpu = vec![DxgiAdapterInfo {
            index: 0,
            name: "Intel(R) Iris(R) Xe Graphics".into(),
            vendor_id: 0x8086,
            dedicated_vram: 128 * MB,
            is_software: false,
            is_high_perf_first: true,
        }];
        let best_single = pick_best_dxgi_adapter(&single_igpu).unwrap();
        assert_eq!(best_single.index, 0);
    }

    #[cfg(windows)]
    #[test]
    #[ignore]
    fn live_dxgi_and_inference_smoke() {
        let state = crate::state::AppState::new();
        detect_device(&state);
        println!("Detected: {} | {:?}", state.device_name(), *state.provider_cfg.read().unwrap());
        let _ = run_inference(&state, vec![0f32; 3 * 1024 * 1024]).unwrap();
        for i in 1..=3 {
            let t = Instant::now();
            let _ = run_inference(&state, vec![0f32; 3 * 1024 * 1024]).unwrap();
            println!("run_inference #{i}: {:.3}s", t.elapsed().as_secs_f32());
        }
    }
}
