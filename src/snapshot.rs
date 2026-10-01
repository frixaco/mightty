//! Frame acquisition stays on GPUI; readback, PNG encoding and bundle I/O use workers.
use crate::{
    control::{ControlError, Request},
    feedback::TerminalCapture,
};
use gpui::{Bounds, GpuCapture, Pixels};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

static ACTIVE: AtomicUsize = AtomicUsize::new(0);
static NEXT: AtomicUsize = AtomicUsize::new(1);
static LIVE_STATE: std::sync::Mutex<Option<Arc<Value>>> = std::sync::Mutex::new(None);
pub fn publish_state(state: Value) {
    *LIVE_STATE.lock().unwrap() = Some(Arc::new(state));
}
pub fn feedback(window: &mut gpui::Window, cx: &mut gpui::App) {
    let mut request = Request {
        protocol_version: 1,
        instance_id: crate::control::instance_id().into(),
        request_id: format!("feedback-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
        op: "snapshot".into(),
        target: Default::default(),
        args: Default::default(),
        preconditions: Default::default(),
        timeout_ms: 5000,
    };
    let acquired = Permit::acquire()
        .and_then(|permit| acquire_presented(&request, window).map(|prepared| (permit, prepared)));
    let (permit, prepared) = match acquired {
        Ok(value) => value,
        Err(error) => {
            crate::diagnostics::record(
                "capture",
                error.code,
                &error.message,
                json!({"request_id":request.request_id}),
            );
            return;
        }
    };
    request.target.window_id = Some(prepared.frame.window_id.clone());
    let state = LIVE_STATE
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| s.as_ref().clone())
        .unwrap_or_else(
            || json!({"availability":"unavailable","reason":"initial state not published"}),
        );
    cx.background_executor()
        .spawn(async move {
            match write(prepared, request, state, permit) {
                Ok(paths) => eprintln!("Feedback capture: {}", paths["directory"]),
                Err(error) => crate::diagnostics::record(
                    "capture",
                    error.code,
                    &error.message,
                    *error.details,
                ),
            }
        })
        .detach();
}
pub struct Permit;
impl Permit {
    pub fn acquire() -> Result<Self, ControlError> {
        ACTIVE
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < 4).then_some(active + 1)
            })
            .map(|_| Self)
            .map_err(|_| ControlError::new("busy", "at most four capture requests"))
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Serialize)]
pub struct PaneFrame {
    pub state: Value,
    pub source: Option<Arc<TerminalCapture>>,
}
#[derive(Serialize)]
pub struct Frame {
    pub schema_version: u32,
    pub instance_id: String,
    pub window_id: String,
    pub scene_revision: String,
    pub prepared_at: String,
    pub dpi_scale: f32,
    pub window: Value,
    pub panes: Vec<PaneFrame>,
    pub labels: Vec<Value>,
}
pub fn rect(bounds: Bounds<Pixels>) -> Value {
    json!({"x":f32::from(bounds.origin.x),"y":f32::from(bounds.origin.y),"width":f32::from(bounds.size.width),"height":f32::from(bounds.size.height)})
}
pub struct Prepared {
    pub gpu: GpuCapture,
    pub frame: Arc<Frame>,
    pub crop: Option<Value>,
    pub mode: String,
    pub visibility: Value,
    pub diagnostics: Vec<Value>,
    pub diagnostics_observed_at: String,
}

pub fn validate_size(size: gpui::Size<Pixels>, scale: f32) -> Result<(), ControlError> {
    let width = f32::from(size.width) * scale;
    let height = f32::from(size.height) * scale;
    if !width.is_finite()
        || !height.is_finite()
        || width <= 0.
        || height <= 0.
        || width.ceil() as u64 * height.ceil() as u64 * 4 > gpui::MAX_CAPTURE_RGBA_BYTES as u64
    {
        return Err(ControlError::new(
            "capture_limit",
            "capture must have positive dimensions and fit 64 MiB RGBA",
        ));
    }
    Ok(())
}
struct Scratch;
impl gpui::Render for Scratch {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        gpui::Empty
    }
}
pub fn scratch_window(
    size: gpui::Size<Pixels>,
    cx: &mut gpui::App,
) -> Result<gpui::AnyWindowHandle, ControlError> {
    use gpui::AppContext;
    let handle = cx
        .open_window(
            gpui::WindowOptions {
                show: false,
                focus: false,
                titlebar: None,
                window_bounds: Some(gpui::WindowBounds::Windowed(Bounds::new(
                    gpui::Point::default(),
                    size,
                ))),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Scratch),
        )
        .map_err(|e| ControlError::new("frame_unavailable", e.to_string()))?;
    Ok(handle.into())
}
pub fn paint_offscreen(
    handle: gpui::AnyWindowHandle,
    element: gpui::AnyElement,
    size: gpui::Size<Pixels>,
    scale: f32,
    cx: &mut gpui::App,
) -> Result<GpuCapture, ControlError> {
    handle
        .update(cx, |_, window, cx| {
            let result = window.capture_element(element, size, scale, cx);
            window.remove_window();
            result.map_err(|e| ControlError::new("frame_unavailable", e.to_string()))
        })
        .map_err(|e| ControlError::new("frame_unavailable", e.to_string()))?
}
pub fn visibility(window: &gpui::Window) -> Value {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};
        if let Ok(handle) = HasWindowHandle::window_handle(window)
            && let RawWindowHandle::Win32(handle) = handle.as_raw()
        {
            let raw = handle.hwnd.get() as windows_sys::Win32::Foundation::HWND;
            return json!({"visible":unsafe{IsWindowVisible(raw)}!=0,"minimized":unsafe{IsIconic(raw)}!=0});
        }
    }
    json!({"visible":null,"minimized":null})
}
pub fn pane_presentation(window: &gpui::Window, pane_id: &str) -> Value {
    if let Some(data) = window.presented_metadata()
        && let Ok(frame) = data.downcast::<Frame>()
        && let Some(pane) = frame
            .panes
            .iter()
            .find(|pane| pane.state["pane_id"] == pane_id)
    {
        return json!({"availability":"observed","frame_id":window.presented_frame_id().to_string(),"bounds":pane.state["computed_bounds"],"scene_revision":frame.scene_revision});
    }
    json!({"availability":"unavailable","reason":"pane absent from latest presented scene"})
}

fn operating_system() -> Value {
    #[cfg(windows)]
    {
        use windows_sys::{
            Wdk::System::SystemServices::RtlGetVersion,
            Win32::System::SystemInformation::OSVERSIONINFOW,
        };
        let mut version = OSVERSIONINFOW {
            dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
            ..Default::default()
        };
        let status = unsafe { RtlGetVersion(&mut version) };
        if status >= 0 {
            json!({"platform":"windows","major":version.dwMajorVersion,"minor":version.dwMinorVersion,"build":version.dwBuildNumber})
        } else {
            json!({"platform":"windows","version":{"availability":"unavailable","ntstatus":status}})
        }
    }
    #[cfg(not(windows))]
    json!({"platform":std::env::consts::OS,"version":{"availability":"unavailable"}})
}
pub fn acquire_presented(
    request: &Request,
    window: &gpui::Window,
) -> Result<Prepared, ControlError> {
    let (gpu, metadata) = window
        .capture_presented()
        .map_err(|e| ControlError::new("frame_unavailable", e.to_string()))?;
    let frame = metadata
        .and_then(|metadata| metadata.downcast::<Frame>().ok())
        .ok_or_else(|| {
            ControlError::new("frame_unavailable", "matching frame metadata unavailable")
        })?;
    let crop = if let Some(id) = request.target.pane_id.as_deref() {
        Some(
            frame
                .panes
                .iter()
                .find(|pane| pane.state["pane_id"] == id)
                .ok_or_else(|| {
                    ControlError::new(
                        "target_not_in_frame",
                        "pane is not present in retained frame",
                    )
                })?
                .state["computed_bounds"]
                .clone(),
        )
    } else if let Some(id) = request.target.tab_id.as_deref() {
        if frame.window["active_tab_id"] != id {
            return Err(ControlError::new(
                "target_not_in_frame",
                "tab is not present in retained frame",
            ));
        }
        Some(frame.window["content_bounds"].clone())
    } else {
        None
    };
    if let Some(expected) = crate::control::string_arg(request, "layout")? {
        let actual = if request.target.tab_id.is_some() || request.target.pane_id.is_some() {
            &frame.window["tab_layout_token"]
        } else {
            &frame.window["layout_token"]
        };
        if actual != expected {
            let mut error = ControlError::new(
                "precondition_failed",
                "captured frame has a different layout token",
            );
            error.details = Box::new(json!({"actual_layout_token":actual}));
            return Err(error);
        }
    }
    Ok(Prepared {
        gpu,
        frame,
        crop,
        mode: crate::control::string_arg(request, "frame")?
            .unwrap_or("presented")
            .into(),
        visibility: visibility(window),
        diagnostics: crate::diagnostics::recent(),
        diagnostics_observed_at: crate::diagnostics::timestamp(),
    })
}

pub fn write(
    prepared: Prepared,
    request: Request,
    state: Value,
    _permit: Permit,
) -> Result<Value, ControlError> {
    let out = crate::control::string_arg(&request, "out")?.map_or_else(
        || std::env::current_dir().unwrap_or_default().join("captures"),
        PathBuf::from,
    );
    if !out.is_absolute() {
        return Err(ControlError::new(
            "invalid_directory",
            "output directory must be absolute",
        ));
    }
    let id = format!(
        "capture-{}-{}-{}",
        crate::feedback::unix_timestamp_ms(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let temporary = out.join(format!(".{id}.tmp"));
    let final_path = out.join(&id);
    let publish = (|| -> std::io::Result<Value> {
        std::fs::create_dir_all(&out)?;
        std::fs::create_dir(&temporary)?;
        let frame_id = prepared.gpu.frame_id;
        let retention = json!({"retained_bytes":prepared.gpu.retained_bytes.to_string(),"copy_submission_ns":prepared.gpu.copy_submission_ns.to_string(),"gpu_copy_duration":{"availability":"unavailable","reason":"GPU timestamp queries are not enabled"},"max_retention_per_window_bytes":(2*gpui::MAX_CAPTURE_RGBA_BYTES).to_string(),"max_readback_bytes":gpui::MAX_CAPTURE_RGBA_BYTES.to_string()});
        let width = prepared.gpu.width;
        let height = prepared.gpu.height;
        let timestamp = prepared
            .gpu
            .timestamp
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let mut statuses = serde_json::Map::new();
        let mut errors = Vec::new();
        let read_start = std::time::Instant::now();
        let pixels = prepared.gpu.read().map_err(|e| e.to_string());
        let readback_ns = read_start.elapsed().as_nanos().to_string();
        let dimensions = match pixels {
            Ok(pixels) => {
                let image = image::RgbaImage::from_raw(width, height, pixels)
                    .ok_or_else(|| std::io::Error::other("invalid readback dimensions"))?;
                let image = if let Some(crop) = &prepared.crop {
                    let scale = prepared.frame.dpi_scale;
                    let x = (crop["x"].as_f64().unwrap() as f32 * scale).floor().max(0.) as u32;
                    let y = (crop["y"].as_f64().unwrap() as f32 * scale).floor().max(0.) as u32;
                    let right = ((crop["x"].as_f64().unwrap() + crop["width"].as_f64().unwrap())
                        as f32
                        * scale)
                        .ceil()
                        .max(0.) as u32;
                    let bottom = ((crop["y"].as_f64().unwrap() + crop["height"].as_f64().unwrap())
                        as f32
                        * scale)
                        .ceil()
                        .max(0.) as u32;
                    if x >= width || y >= height || right <= x || bottom <= y {
                        return Err(std::io::Error::other("crop outside frame"));
                    }
                    image::imageops::crop_imm(
                        &image,
                        x,
                        y,
                        right.min(width) - x,
                        bottom.min(height) - y,
                    )
                    .to_image()
                } else {
                    image
                };
                let dimensions = (image.width(), image.height());
                match image.save(temporary.join("image.png")) {
                    Ok(()) => {
                        statuses.insert("image.png".into(), json!({"status":"complete"}));
                    }
                    Err(error) => {
                        errors.push(error.to_string());
                        statuses.insert(
                            "image.png".into(),
                            json!({"status":"error","message":error.to_string()}),
                        );
                    }
                }
                Some(dimensions)
            }
            Err(error) => {
                errors.push(error.clone());
                statuses.insert(
                    "image.png".into(),
                    json!({"status":"error","message":error}),
                );
                None
            }
        };
        let environment = json!({"build":{"mightty_version":env!("CARGO_PKG_VERSION"),"mightty_revision":env!("MIGHTTY_BUILD_REVISION"),"target":env!("MIGHTTY_BUILD_TARGET"),"profile":env!("MIGHTTY_BUILD_PROFILE"),"ghostty_headers":env!("MIGHTTY_GHOSTTY_HEADERS"),"embedded_fonts":env!("MIGHTTY_FONT_FINGERPRINTS"),"ghostty_revision":crate::ghostty::SOURCE_REVISION,"gpui_version":"0.2.2 with local capture patch","debug_assertions":cfg!(debug_assertions)},"os":operating_system(),"backend":"Direct3D11","color":{"source":"BGRA8 UNORM","export":"RGBA8 PNG","conversion":"lossless channel swizzle; no color correction"},"dpi_scale":prepared.frame.dpi_scale,"frame_effective_settings":prepared.frame.window["settings"],"gpu":prepared.frame.window["gpu"],"current_settings_generations":state["windows"].as_array().map(|windows|windows.iter().map(|window|json!({"window_id":window["window_id"],"generation":window["settings_generation"]})).collect::<Vec<_>>()),"launch_recipes":prepared.frame.panes.iter().map(|pane|&pane.state["launch"]).collect::<Vec<_>>()});
        for (name, value) in [
            (
                "frame.json",
                serde_json::to_value(&prepared.frame).map_err(std::io::Error::other)?,
            ),
            ("state.json", state.clone()),
            ("environment.json", environment),
        ] {
            match std::fs::write(
                temporary.join(name),
                serde_json::to_vec_pretty(&value).map_err(std::io::Error::other)?,
            ) {
                Ok(()) => {
                    statuses.insert(name.into(), json!({"status":"complete"}));
                }
                Err(error) => {
                    errors.push(error.to_string());
                    statuses.insert(
                        name.into(),
                        json!({"status":"error","message":error.to_string()}),
                    );
                }
            }
        }
        let diagnostics = prepared
            .diagnostics
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        match std::fs::write(temporary.join("diagnostics.ndjson"), diagnostics) {
            Ok(()) => {
                statuses.insert("diagnostics.ndjson".into(), json!({"status":"complete"}));
            }
            Err(error) => {
                errors.push(error.to_string());
                statuses.insert(
                    "diagnostics.ndjson".into(),
                    json!({"status":"error","message":error.to_string()}),
                );
            }
        }
        if let Err(error) = std::fs::create_dir(temporary.join("panes")) {
            errors.push(error.to_string());
        }
        for pane in &prepared.frame.panes {
            if let (Some(id), Some(source)) = (pane.state["pane_id"].as_str(), &pane.source) {
                let name = format!("panes/{id}.txt");
                let text = source
                    .rows
                    .iter()
                    .map(|row| row.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                match std::fs::write(temporary.join(&name), text) {
                    Ok(()) => {
                        statuses.insert(name, json!({"status":"complete"}));
                    }
                    Err(error) => {
                        errors.push(error.to_string());
                        statuses
                            .insert(name, json!({"status":"error","message":error.to_string()}));
                    }
                }
            } else if let Some(id) = pane.state["pane_id"].as_str() {
                statuses.insert(format!("panes/{id}.txt"), json!({"status":"unavailable","reason":"source cells unavailable or over budget"}));
            }
        }
        let status = if errors.is_empty() {
            "complete"
        } else {
            "partial"
        };
        let mut manifest = json!({"schema_version":1,"protocol_version":1,"instance_id":request.instance_id,"request_id":request.request_id,"target":request.target,"mode":prepared.mode,"frame_id":if prepared.mode=="offscreen" {format!("offscreen:{}",request.request_id)} else {frame_id.to_string()},"resource_cost":retention,"readback_ns":readback_ns,"frame_revision":prepared.frame.scene_revision,"frame_revision_domain":if prepared.mode=="offscreen"{"offscreen_preparation"}else{"window_scene"},"live_revision":state["revision"],"live_revision_domain":"application","frame_time_unix_ms":timestamp.to_string(),"frame_age_ms":crate::feedback::unix_timestamp_ms().saturating_sub(timestamp).to_string(),"visibility":prepared.visibility,"dimensions":dimensions,"dpi_scale":prepared.frame.dpi_scale,"artifacts":statuses,"diagnostics":{"observed_at":prepared.diagnostics_observed_at,"limit":128,"count":prepared.diagnostics.len(),"range":{"first":prepared.diagnostics.first().map(|v|&v["time"]),"last":prepared.diagnostics.last().map(|v|&v["time"])},"truncated":prepared.diagnostics.len()==128},"output_cursors":prepared.frame.panes.iter().map(|pane|&pane.state["output_cursor"]).collect::<Vec<_>>(),"shaping":{"availability":"unavailable","reason":"GPUI does not retain face and glyph cluster diagnostics"},"source_cells":{"max_cells_per_pane":20000,"max_cells_per_frame":40000,"omitted_pane_ids":prepared.frame.panes.iter().filter(|pane|pane.source.is_none()).map(|pane|&pane.state["pane_id"]).collect::<Vec<_>>()},"status":status,"errors":errors});
        manifest["crop_bounds_logical"] = prepared.crop.clone().unwrap_or(Value::Null);
        manifest["source_frame_dimensions"] = json!({"width":width,"height":height});
        std::fs::write(
            temporary.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(&temporary, &final_path)?;
        Ok(
            json!({"directory":final_path,"manifest":final_path.join("manifest.json"),"image":final_path.join("image.png"),"status":status}),
        )
    })();
    match publish {
        Ok(value) if value["status"] == "complete" => Ok(value),
        Ok(value) => {
            let mut error = ControlError::new("capture_partial", "capture bundle contains errors");
            error.effect = "partial";
            error.details = Box::new(value);
            Err(error)
        }
        Err(error) => {
            let mut result = ControlError::new("capture_failed", error.to_string());
            result.effect = "partial";
            result.details = Box::new(json!({"temporary_directory":temporary}));
            Err(result)
        }
    }
}
