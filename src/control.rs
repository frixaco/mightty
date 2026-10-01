//! Local CLI protocol. Application owners resolve live targets on GPUI.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{self, Read},
    path::PathBuf,
};
static TEST_DATA_DIRECTORY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
static INSTANCE_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn set_test_directory(path: PathBuf) -> Result<(), String> {
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    TEST_DATA_DIRECTORY
        .set(path)
        .map_err(|_| "test directory already initialized".to_string())
}
pub fn test_directory() -> Option<&'static std::path::Path> {
    TEST_DATA_DIRECTORY.get().map(PathBuf::as_path)
}
pub fn set_instance_id(id: String) {
    let _ = INSTANCE_ID.set(id);
}
pub fn instance_id() -> &'static str {
    INSTANCE_ID
        .get()
        .map(String::as_str)
        .unwrap_or("uninitialized")
}

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_INPUT_BYTES: usize = 256 * 1024;
pub const MAX_READ_ROWS: usize = 2000;
pub const MAX_CONNECTIONS: usize = 8;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub window_id: Option<String>,
    pub tab_id: Option<String>,
    pub pane_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub protocol_version: u32,
    pub request_id: String,
    pub instance_id: String,
    pub op: String,
    pub target: Target,
    pub args: BTreeMap<String, Value>,
    pub preconditions: Preconditions,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preconditions {
    pub layout_token: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Descriptor {
    pub protocol_version: u32,
    pub instance_id: String,
    pub pid: u32,
    pub process_creation_time: String,
    pub endpoint: String,
    pub started_unix_ms: String,
}

pub struct Dispatch {
    pub request: Request,
    pub reply: flume::Sender<Value>,
    pub deadline: std::time::Instant,
}

#[derive(Clone, Debug, Default)]
pub struct Acknowledgement {
    pub written_bytes: usize,
    pub error: Option<String>,
}

pub async fn complete_acknowledgements(
    mut response: Value,
    acknowledgements: Vec<flume::Receiver<Acknowledgement>>,
    deadline: std::time::Instant,
) -> Value {
    let mut written = 0usize;
    for ack in acknowledgements {
        loop {
            match ack.try_recv() {
                Ok(ack) => {
                    written = written.saturating_add(ack.written_bytes);
                    if let Some(error) = ack.error {
                        response["ok"] = json!(false);
                        response["error"] = json!({"code":"pty_error","message":error,"effect":"partial","details":{"written_bytes":written,"operation_result":response["result"],"completed_steps":completed_steps(&response,written)}});
                        response.as_object_mut().unwrap().remove("result");
                        return response;
                    }
                    break;
                }
                Err(flume::TryRecvError::Empty) if std::time::Instant::now() < deadline => {
                    gpui::Timer::after(std::time::Duration::from_millis(5)).await;
                }
                _ => {
                    response["ok"] = json!(false);
                    response["error"] = json!({"code":"outcome_unknown","message":"PTY completion unavailable before deadline","effect":"unknown","details":{"known_written_bytes":written,"operation_result":response["result"]}});
                    response.as_object_mut().unwrap().remove("result");
                    return response;
                }
            }
        }
    }
    let steps = completed_steps(&response, written);
    response["result"]["completion"] =
        json!({"pty_acknowledged":true,"written_bytes":written,"completed_steps":steps});
    response
}
fn completed_steps(response: &Value, written: usize) -> usize {
    response["result"]["step_byte_ends"]
        .as_array()
        .map_or(0, |ends| {
            ends.iter()
                .take_while(|end| end.as_u64().is_some_and(|end| end <= written as u64))
                .count()
        })
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputStep {
    Text {
        text: String,
    },
    Key {
        key: String,
        #[serde(default)]
        modifiers: Vec<String>,
        #[serde(default)]
        event: KeyEvent,
    },
}
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyEvent {
    #[default]
    Tap,
    Press,
    Repeat,
    Release,
}

pub fn keystroke(key: &str, modifiers: &[String]) -> Result<gpui::Keystroke, ControlError> {
    let canonical = if key.chars().count() == 1 {
        key.to_string()
    } else {
        key.to_ascii_lowercase()
    };
    if canonical.chars().count() != 1
        && !matches!(
            canonical.as_str(),
            "enter"
                | "escape"
                | "tab"
                | "backspace"
                | "delete"
                | "insert"
                | "left"
                | "right"
                | "up"
                | "down"
                | "home"
                | "end"
                | "pageup"
                | "pagedown"
                | "space"
                | "f1"
                | "f2"
                | "f3"
                | "f4"
                | "f5"
                | "f6"
                | "f7"
                | "f8"
                | "f9"
                | "f10"
                | "f11"
                | "f12"
        )
    {
        return Err(ControlError::new(
            "invalid_argument",
            format!("unknown key {key}"),
        ));
    }
    let mut mods = gpui::Modifiers::default();
    for modifier in modifiers {
        match modifier.as_str() {
            "ctrl" => mods.control = true,
            "alt" => mods.alt = true,
            "shift" => mods.shift = true,
            "super" => mods.platform = true,
            _ => {
                return Err(ControlError::new(
                    "invalid_argument",
                    format!("unknown modifier {modifier}"),
                ));
            }
        }
    }
    Ok(gpui::Keystroke {
        modifiers: mods,
        key: canonical,
        key_char: None,
    })
}

pub fn direction(value: &str) -> Result<crate::action::Direction, ControlError> {
    use crate::action::Direction;
    match value {
        "left" => Ok(Direction::Left),
        "right" => Ok(Direction::Right),
        "up" => Ok(Direction::Up),
        "down" => Ok(Direction::Down),
        _ => Err(ControlError::new(
            "invalid_argument",
            "direction must be left, right, up or down",
        )),
    }
}

pub fn reply(request: &Request, revision: u64, result: Result<Value, ControlError>) -> Value {
    let mut value = json!({"protocol_version": PROTOCOL_VERSION,
        "request_id": request.request_id, "instance_id": request.instance_id,
        "revision": revision.to_string(), "ok": result.is_ok()});
    match result {
        Ok(result) => value["result"] = result,
        Err(error) => {
            value["error"] = json!({"code": error.code, "message": error.message,
            "effect": error.effect, "details": error.details})
        }
    }
    value
}

#[derive(Debug)]
pub struct ControlError {
    pub code: &'static str,
    pub message: String,
    pub effect: &'static str,
    pub details: Box<Value>,
}
impl ControlError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            effect: "none",
            details: Box::new(json!({})),
        }
    }
}
impl From<crate::ghostty::Error> for ControlError {
    fn from(value: crate::ghostty::Error) -> Self {
        Self::new("terminal_error", value.to_string())
    }
}

// One small operation table drives validation, CLI help and capabilities.
pub const OPERATIONS: &[(&str, &[&str])] = &[
    ("handshake", &[]),
    ("capabilities", &[]),
    ("profiles", &[]),
    ("state", &[]),
    ("pane.read", &["viewport", "tail", "buffer", "format"]),
    (
        "tab.new",
        &[
            "profile",
            "cwd",
            "exec",
            "argv",
            "env",
            "unset_env",
            "env_mode",
            "focus",
        ],
    ),
    ("tab.select", &[]),
    ("tab.move", &["before"]),
    ("tab.close", &[]),
    (
        "pane.split",
        &[
            "profile",
            "cwd",
            "exec",
            "argv",
            "env",
            "unset_env",
            "env_mode",
            "focus",
            "direction",
            "ratio",
        ],
    ),
    ("pane.resize", &["edge", "delta_px"]),
    ("pane.focus", &["direction"]),
    ("pane.zoom", &["enabled"]),
    ("pane.close", &[]),
    ("pane.scroll", &["rows", "to"]),
    ("pane.send-text", &["text"]),
    ("pane.send-key", &["key", "modifiers", "event"]),
    ("pane.input", &["steps"]),
    ("window.resize", &["width", "height"]),
    ("window.focus", &[]),
    ("events", &[]),
    ("snapshot", &["frame", "out", "layout"]),
    ("ui.sidebar", &["visible"]),
    ("ui.palette", &["open", "query"]),
    ("ui.search", &["open", "query"]),
    ("ui.key", &["key", "modifiers", "event"]),
    ("ui.text", &["text"]),
    (
        "ui.pointer",
        &[
            "event",
            "x",
            "y",
            "button",
            "modifiers",
            "delta_x",
            "delta_y",
        ],
    ),
    ("ui.input", &["steps"]),
    (
        "wait",
        &[
            "text",
            "after_output",
            "condition",
            "value",
            "layout",
            "at_least",
            "overlay",
        ],
    ),
];

pub fn validate(request: &Request, instance_id: &str) -> Result<(), ControlError> {
    if request.protocol_version != PROTOCOL_VERSION {
        return Err(ControlError::new(
            "unsupported_version",
            "unsupported control protocol",
        ));
    }
    if request.instance_id != instance_id {
        return Err(ControlError::new(
            "stale_instance",
            "instance identity does not match",
        ));
    }
    if matches!(
        request.op.as_str(),
        "capabilities" | "handshake" | "events" | "profiles"
    ) && (request.target.window_id.is_some()
        || request.target.tab_id.is_some()
        || request.target.pane_id.is_some())
    {
        return Err(ControlError::new(
            "invalid_target",
            "instance-wide operation accepts no object selectors",
        ));
    }
    if request.request_id.is_empty()
        || request.request_id.len() > 128
        || !(1..=60_000).contains(&request.timeout_ms)
    {
        return Err(ControlError::new(
            "invalid_argument",
            "invalid request ID or timeout (1..60000 ms)",
        ));
    }
    let Some((_, arguments)) = OPERATIONS.iter().find(|(op, _)| *op == request.op) else {
        return Err(ControlError::new(
            "unsupported_operation",
            format!("unsupported operation {}", request.op),
        ));
    };
    for id in [
        &request.target.window_id,
        &request.target.tab_id,
        &request.target.pane_id,
        &request.preconditions.layout_token,
    ]
    .into_iter()
    .flatten()
    {
        if id.is_empty() || id.len() > 256 || id.contains('\0') {
            return Err(ControlError::new(
                "invalid_argument",
                "invalid target or layout token",
            ));
        }
    }
    if request.preconditions.layout_token.is_some()
        && matches!(
            request.op.as_str(),
            "state"
                | "profiles"
                | "capabilities"
                | "handshake"
                | "pane.read"
                | "wait"
                | "events"
                | "snapshot"
                | "window.focus"
        )
    {
        return Err(ControlError::new(
            "invalid_argument",
            "this operation does not accept if-layout; capture/wait use layout",
        ));
    }
    for required in required_arguments(&request.op) {
        if !request.args.contains_key(*required) {
            return Err(ControlError::new(
                "invalid_argument",
                format!("{required} required"),
            ));
        }
    }
    for (name, value) in &request.args {
        if !arguments.contains(&name.as_str()) {
            return Err(ControlError::new(
                "invalid_argument",
                format!("unknown argument {name}"),
            ));
        }
        let schema = argument_schema(&request.op, name);
        let valid = match schema["type"].as_str().unwrap() {
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "integer" => value.as_u64().is_some(),
            "number" => value.as_f64().is_some_and(f64::is_finite),
            "array" => value.is_array(),
            _ => false,
        };
        if !valid
            || schema["enum"]
                .as_array()
                .is_some_and(|values| !values.contains(value))
        {
            return Err(ControlError::new(
                "invalid_argument",
                format!("invalid {name}"),
            ));
        }
        if name == "text" && value.as_str().unwrap().len() > MAX_INPUT_BYTES {
            return Err(ControlError::new(
                "input_limit",
                "source text exceeds 256 KiB",
            ));
        }
        if matches!(name.as_str(), "argv" | "env" | "unset_env" | "modifiers") {
            let values = value.as_array().unwrap();
            if values.len() > 256
                || values.iter().any(|v| !v.is_string())
                || values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::len)
                    .sum::<usize>()
                    > 65536
            {
                return Err(ControlError::new(
                    "invalid_argument",
                    format!("{name} requires bounded strings"),
                ));
            }
        }
    }
    Ok(())
}

fn argument_schema(op: &str, name: &str) -> Value {
    let kind = match name {
        "viewport" | "focus" | "enabled" | "visible" | "open" => "boolean",
        "ratio" | "delta_px" | "rows" | "width" | "height" | "x" | "y" | "delta_x" | "delta_y" => {
            "number"
        }
        "tail" => "integer",
        "argv" | "env" | "unset_env" | "modifiers" | "steps" => "array",
        _ => "string",
    };
    let mut schema = json!({"type":kind});
    if kind == "array" {
        schema["items"] = if name == "steps" {
            json!({"$ref":if op.starts_with("ui."){"#/$defs/ui_step"}else{"#/$defs/terminal_step"}})
        } else {
            json!({"type":"string"})
        };
    }
    let values: &[&str] = match name {
        "direction" | "edge" => &["left", "right", "up", "down"],
        "env_mode" => &["inherit", "empty"],
        "buffer" => &["active", "primary", "alternate"],
        "format" => &["text", "cells"],
        "frame" => &["presented", "next", "offscreen"],
        "to" => &["top", "bottom"],
        "button" => &["left", "right", "middle"],
        "overlay" => &["palette", "search"],
        "condition" => &[
            "text",
            "title-equals",
            "layout-ready",
            "settings-generation",
            "overlay-open",
            "prompt-ready",
            "process-exited",
        ],
        "event" if op == "ui.pointer" => &["move", "press", "release", "wheel"],
        "event" if op == "ui.key" => &["tap"],
        "event" => &["tap", "press", "repeat", "release"],
        _ => &[],
    };
    if !values.is_empty() {
        schema["enum"] = json!(values);
    }
    if name == "tail" {
        schema["minimum"] = json!(1);
        schema["maximum"] = json!(MAX_READ_ROWS);
    }
    if name == "steps" {
        schema["minItems"] = json!(1);
        schema["maxItems"] = json!(1024);
    }
    schema
}
fn required_arguments(op: &str) -> &'static [&'static str] {
    match op {
        "pane.split" => &["direction"],
        "pane.resize" => &["edge", "delta_px"],
        "pane.send-text" | "ui.text" => &["text"],
        "pane.send-key" | "ui.key" => &["key"],
        "pane.input" | "ui.input" => &["steps"],
        "window.resize" => &["width", "height"],
        "tab.move" => &["before"],
        "ui.pointer" => &["event", "x", "y"],
        _ => &[],
    }
}
fn result_schema(op: &str) -> Value {
    match op {
        "profiles" => {
            json!({"type":"array","items":{"type":"object","required":["profile_id","label","executable","argv"]}})
        }
        "pane.read" => {
            json!({"type":"object","required":["text","rows","buffer","output_cursor","truncation"]})
        }
        "snapshot" => {
            json!({"type":"object","required":["directory","manifest","image","status"],"properties":{"status":{"enum":["complete","partial"]}}})
        }
        _ => json!({"type":"object","additionalProperties":true}),
    }
}
pub fn capabilities() -> Value {
    json!({"protocol_version": PROTOCOL_VERSION, "operations": OPERATIONS.iter().map(|(op,args)|
        json!({"op":op,"arguments":args,"argument_schema":{"type":"object","properties":args.iter().map(|name|((*name).to_string(),argument_schema(op,name))).collect::<BTreeMap<_,_>>(),"required":required_arguments(op),"additionalProperties":false},"result_schema":result_schema(op),"error_schema":{"$ref":"#/$defs/error"},"availability":if cfg!(windows){"supported"}else{"unsupported"}})).collect::<Vec<_>>(), "platform":std::env::consts::OS,
        "$defs":{
            "request":{"type":"object","required":["protocol_version","request_id","instance_id","op","target","args","preconditions","timeout_ms"],"properties":{"protocol_version":{"const":1},"request_id":{"type":"string","minLength":1,"maxLength":128},"instance_id":{"type":"string"},"op":{"enum":OPERATIONS.iter().map(|(op,_)|*op).collect::<Vec<_>>()},"target":{"type":"object","properties":{"window_id":{"type":["string","null"]},"tab_id":{"type":["string","null"]},"pane_id":{"type":["string","null"]}},"additionalProperties":false},"args":{"type":"object","description":"Validated against the selected operation argument_schema"},"preconditions":{"type":"object","properties":{"layout_token":{"type":["string","null"]}},"additionalProperties":false},"timeout_ms":{"type":"integer","minimum":1,"maximum":60000}},"additionalProperties":false},
            "event":{"type":"object","required":["protocol_version","instance_id","revision","type"],"properties":{"protocol_version":{"const":1},"instance_id":{"type":"string"},"revision":{"type":["string","null"]},"type":{"enum":["state","change","resync_required","end"]},"changes":{"type":"array","items":{"type":"object","required":["kind"]}},"state":{"type":"object"},"last_delivered_revision":{"type":["string","null"]}}},
            "error":{"type":"object","required":["code","message","effect","details"],"properties":{"code":{"type":"string"},"message":{"type":"string"},"effect":{"enum":["none","committed","partial","unknown"]},"details":{"type":"object"}}},
            "terminal_step":{"oneOf":[{"type":"object","required":["type","text"],"properties":{"type":{"const":"text"},"text":{"type":"string"}},"additionalProperties":false},{"type":"object","required":["type","key"],"properties":{"type":{"const":"key"},"key":{"type":"string"},"modifiers":{"type":"array","items":{"enum":["ctrl","alt","shift","super"]}},"event":{"enum":["tap","press","repeat","release"]}},"additionalProperties":false}]},
            "ui_step":{"oneOf":[{"$ref":"#/$defs/terminal_step"},{"type":"object","required":["type","event","x","y"],"properties":{"type":{"const":"pointer"},"event":{"enum":["move","press","release","wheel"]},"x":{"type":"number"},"y":{"type":"number"},"button":{"enum":["left","right","middle"]},"modifiers":{"type":"array","items":{"enum":["ctrl","alt","shift","super"]}},"delta_x":{"type":"number"},"delta_y":{"type":"number"}},"additionalProperties":false}]},
            "reply":{"type":"object","required":["protocol_version","request_id","instance_id","revision","ok"],"properties":{"protocol_version":{"const":1},"request_id":{"type":["string","null"]},"instance_id":{"type":["string","null"]},"revision":{"type":["string","null"]},"ok":{"type":"boolean"},"result":{},"error":{"$ref":"#/$defs/error"}}}
        },
        "capture":{"backend":if cfg!(windows){"Direct3D11"}else{"unsupported"},"modes":["presented","next","offscreen"],"offscreen_targets":["tab","pane"],"optional_shaping":"unavailable"},
        "terminal_presentation":{"synchronized_output":{"mode":2026,"host_support":true,"maximum_hold_ms":1000,"recovery":["timeout","resize","reset","eof"]},"cursor":{"visibility":true,"application_shape_and_blink":true,"wide_cells":true},"graphics":{"direct_pixel_placements":true,"virtual_placements":false}},
        "limits":{"frame_bytes":MAX_FRAME_BYTES,"input_bytes":MAX_INPUT_BYTES,"input_steps":1024,"ui_sequences":1,"capture_requests":4,"capture_surface_bytes":gpui::MAX_CAPTURE_RGBA_BYTES,"capture_retention_bytes_per_window":2*gpui::MAX_CAPTURE_RGBA_BYTES,"capture_source_cells_per_frame":40000,"capture_source_cells_per_pane":20000,"read_rows":MAX_READ_ROWS,"connections":MAX_CONNECTIONS,"request_queue":32,"timeout_ms":60000,"event_queue":32,"removed_panes":64,"removed_pane_retention_seconds":300,"journal_bytes":4*1024*1024,"journal_files":2,"inactive_metadata_instances":32,"launch_list_items":256,"launch_list_bytes":65536},
        "constraints":{"input":"Entire sequences must balance held keys/buttons; cancellation releases only synthetic held state.","ui_pointer":"Single pointer commands support move/wheel; use ui.input for presses and drags.","paste":"Unsafe paste requires confirmation and is rejected by unattended input.","read":"Only the active terminal buffer is observable; inactive buffers are never switched.","text_wait":"after_output gates progress, not text freshness; use unique composed markers.","events":"Initial full state followed by atomic replacement changes; overflow requires resubscription.","snapshot":"Presented never redraws. Tab/pane default offscreen; OS popups are excluded."}})
}

pub fn string_arg<'a>(request: &'a Request, name: &str) -> Result<Option<&'a str>, ControlError> {
    request
        .args
        .get(name)
        .map(|value| {
            value.as_str().ok_or_else(|| {
                ControlError::new("invalid_argument", format!("{name} must be text"))
            })
        })
        .transpose()
}
pub fn bool_arg(request: &Request, name: &str, default: bool) -> Result<bool, ControlError> {
    request.args.get(name).map_or(Ok(default), |v| {
        v.as_bool()
            .ok_or_else(|| ControlError::new("invalid_argument", format!("{name} must be boolean")))
    })
}
pub fn number_arg(request: &Request, name: &str, default: f64) -> Result<f64, ControlError> {
    request.args.get(name).map_or(Ok(default), |v| {
        v.as_f64()
            .filter(|v| v.is_finite())
            .ok_or_else(|| ControlError::new("invalid_argument", format!("{name} must be finite")))
    })
}

pub fn directory() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("mightty")
        .join("control")
}

pub fn read_utf8(path: &str, limit: usize) -> Result<String, String> {
    let input: Box<dyn Read> = if path == "-" {
        Box::new(io::stdin())
    } else {
        Box::new(std::fs::File::open(path).map_err(|e| e.to_string())?)
    };
    let mut bytes = Vec::new();
    input
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err(format!("input exceeds {limit} bytes"));
    }
    let text = String::from_utf8(bytes).map_err(|_| "input must be UTF-8".to_string())?;
    Ok(text.strip_prefix('\u{feff}').unwrap_or(&text).to_string())
}

pub fn cli(arguments: Vec<String>) -> i32 {
    let json_output = arguments
        .iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| arg == "--json");
    match run_cli(arguments) {
        Ok(value) => {
            let ok = value["ok"].as_bool().unwrap_or(true);
            if json_output {
                println!("{}", serde_json::to_string(&value).unwrap());
            } else if !ok {
                eprintln!(
                    "{}: {} (effect: {})",
                    value["error"]["code"].as_str().unwrap_or("error"),
                    value["error"]["message"]
                        .as_str()
                        .unwrap_or("control failed"),
                    value["error"]["effect"].as_str().unwrap_or("unknown")
                );
            } else if let Some(usage) = value["usage"].as_str() {
                println!("{usage}");
                for (op, args) in OPERATIONS {
                    println!(
                        "  {} {}",
                        op.replace('.', " "),
                        args.iter()
                            .map(|name| format!("--{}", name.replace('_', "-")))
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                }
                println!(
                    "Use --file PATH or --file - for text/input. Repeated --mod, --env and --unset-env are supported. Literal exec argv follows --. Timeouts accept ms or s."
                );
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(value.get("result").unwrap_or(&value)).unwrap()
                );
            }
            if ok { 0 } else { 1 }
        }
        Err(error) => {
            if json_output {
                println!(
                    "{}",
                    json!({"protocol_version":PROTOCOL_VERSION,"request_id":null,
                "instance_id":null,"revision":null,"ok":false,
                "error":{"code":"client_error","effect":"none","message":error,"details":{}}})
                );
            } else {
                eprintln!("{error}");
            }
            1
        }
    }
}

fn run_cli(arguments: Vec<String>) -> Result<Value, String> {
    if arguments.is_empty()
        || arguments
            .iter()
            .take_while(|arg| arg.as_str() != "--")
            .any(|a| a == "--help" || a == "-h")
    {
        return Ok(
            json!({"usage":"mightty ctl OPERATION [--instance ID] [--window ID] [--tab ID] [--pane ID] [--json]",
            "operations":OPERATIONS.iter().map(|(op,args)|json!({"op":op,"arguments":args})).collect::<Vec<_>>() }),
        );
    }
    let (mut request, instance, json_output) = parse_cli(arguments)?;
    if request.op == "state" && bool_arg(&request, "saved", false).map_err(|e| e.message)? {
        if request
            .args
            .keys()
            .any(|name| !matches!(name.as_str(), "saved" | "data_dir"))
            || request.preconditions.layout_token.is_some()
            || request.target.window_id.is_some()
            || request.target.tab_id.is_some()
            || request.target.pane_id.is_some()
        {
            return Err("saved state accepts only instance and data-dir selectors".into());
        }
        let state = crate::diagnostics::read_saved(
            instance.as_deref(),
            string_arg(&request, "data_dir").map_err(|e| e.message)?,
        )?;
        return Ok(
            json!({"protocol_version":1,"request_id":request.request_id,"instance_id":state["instance_id"],"revision":state["revision"],"ok":true,"result":state}),
        );
    }
    if request.op != "instances" {
        validate(&request, &request.instance_id).map_err(|e| e.message)?;
    } else if !request.args.is_empty()
        || request.preconditions.layout_token.is_some()
        || request.target.window_id.is_some()
        || request.target.tab_id.is_some()
        || request.target.pane_id.is_some()
    {
        return Err("instances accepts no object selectors or arguments".into());
    }
    #[cfg(windows)]
    {
        let descriptors =
            crate::application::windows::discover_control_instances(if request.op == "instances" {
                None
            } else {
                instance.as_deref()
            })
            .map_err(|e| e.to_string())?;
        if request.op == "instances" {
            return Ok(
                json!({"protocol_version":PROTOCOL_VERSION,"request_id":request.request_id,"instance_id":null,"revision":null,"ok":true,"result":descriptors}),
            );
        }
        let descriptor = if let Some(instance) = instance {
            descriptors
                .iter()
                .find(|d| d.instance_id == instance)
                .ok_or_else(|| format!("instance {instance} unavailable"))?
        } else {
            match descriptors.as_slice() {
                [only] => only,
                [] => return Err("no running mightty instance".to_string()),
                _ => {
                    return Err(format!(
                        "ambiguous instance: {}",
                        serde_json::to_string(&descriptors).unwrap()
                    ));
                }
            }
        };
        request.instance_id = descriptor.instance_id.clone();
        let value = match crate::application::windows::send_control(descriptor, &request) {
            Ok(value) => value,
            Err(error) => {
                let mut failure = ControlError::new("outcome_unknown", error.to_string());
                failure.effect = "unknown";
                let mut value = reply(&request, 0, Err(failure));
                value["revision"] = Value::Null;
                return Ok(value);
            }
        };
        if request.op == "events" {
            std::process::exit(
                if value["ok"] == false || value["type"] == "resync_required" {
                    1
                } else {
                    0
                },
            );
        }
        if !json_output
            && request.op == "pane.read"
            && value["ok"] == true
            && string_arg(&request, "format").ok().flatten() != Some("cells")
        {
            print!("{}", value["result"]["text"].as_str().unwrap_or(""));
            std::process::exit(0);
        }
        Ok(value)
    }
    #[cfg(not(windows))]
    {
        let _ = (&mut request, instance, json_output);
        Err("control transport currently requires Windows".into())
    }
}

fn parse_cli(arguments: Vec<String>) -> Result<(Request, Option<String>, bool), String> {
    let mut iter = arguments.into_iter().peekable();
    let mut op = iter.next().unwrap();
    if matches!(op.as_str(), "pane" | "tab" | "window" | "ui") {
        op.push('.');
        op.push_str(&iter.next().ok_or("missing operation")?);
    }
    let mut explicit = Target::default();
    let mut instance = None;
    let mut args = BTreeMap::new();
    let mut preconditions = Preconditions::default();
    let mut timeout_ms = 5000;
    let mut json_output = false;
    while let Some(option) = iter.next() {
        if option == "--" {
            args.insert("argv".into(), json!(iter.collect::<Vec<_>>()));
            break;
        }
        if option == "--json" {
            json_output = true;
            continue;
        }
        if matches!(option.as_str(), "--viewport" | "--focus" | "--saved") {
            args.insert(option[2..].to_string(), json!(true));
            continue;
        }
        let name = option
            .strip_prefix("--")
            .ok_or_else(|| format!("unexpected argument {option}"))?
            .replace('-', "_");
        let value = iter
            .next()
            .ok_or_else(|| format!("missing value for {option}"))?;
        match name.as_str() {
            "instance" => instance = Some(value),
            "window" => explicit.window_id = Some(value),
            "tab" => explicit.tab_id = Some(value),
            "pane" => explicit.pane_id = Some(value),
            "if_layout" => preconditions.layout_token = Some(value),
            "timeout" => timeout_ms = parse_timeout(&value)?,
            "file" => {
                let text = read_utf8(&value, MAX_INPUT_BYTES)?;
                match op.as_str() {
                    "pane.send-text" | "ui.text" => {
                        args.insert("text".into(), json!(text));
                    }
                    "pane.input" | "ui.input" => {
                        args.insert(
                            "steps".into(),
                            serde_json::from_str(&text)
                                .map_err(|e| format!("invalid input JSON: {e}"))?,
                        );
                    }
                    _ => return Err("--file is only supported for input".into()),
                }
            }
            "mod" | "env" | "unset_env" => {
                let key = if name == "mod" { "modifiers" } else { &name };
                args.entry(key.into())
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                    .unwrap()
                    .push(json!(value));
            }
            "cwd" | "out" | "data_dir" => {
                let path = PathBuf::from(value);
                let path = if path.is_absolute() {
                    path
                } else {
                    std::env::current_dir()
                        .map_err(|e| e.to_string())?
                        .join(path)
                };
                args.insert(name, json!(path));
            }
            "enabled" | "visible" | "open" => {
                args.insert(
                    name,
                    json!(
                        value
                            .parse::<bool>()
                            .map_err(|_| "expected true or false")?
                    ),
                );
            }
            "ratio" | "delta_px" | "rows" | "width" | "height" | "x" | "y" | "delta_x"
            | "delta_y" => {
                args.insert(
                    name,
                    json!(value.parse::<f64>().map_err(|_| "expected finite number")?),
                );
            }
            "tail" => {
                args.insert(
                    name,
                    json!(
                        value
                            .parse::<usize>()
                            .map_err(|_| "tail must be an integer")?
                    ),
                );
            }
            _ => {
                if args.insert(name.clone(), json!(value)).is_some() {
                    return Err(format!("duplicate argument {name}"));
                }
            }
        }
    }
    let inherited_instance = std::env::var("MIGHTTY_INSTANCE_ID").ok();
    let use_inherited = instance
        .as_ref()
        .is_none_or(|id| Some(id) == inherited_instance.as_ref());
    let target = if explicit.window_id.is_none()
        && explicit.tab_id.is_none()
        && explicit.pane_id.is_none()
        && !matches!(
            op.as_str(),
            "state" | "capabilities" | "profiles" | "instances" | "events"
        )
        && use_inherited
    {
        Target {
            window_id: None,
            tab_id: None,
            pane_id: std::env::var("MIGHTTY_PANE_ID").ok(),
        }
    } else {
        explicit
    };
    let instance = if args.get("saved") == Some(&json!(true)) {
        instance
    } else {
        instance.or(inherited_instance)
    };
    Ok((
        Request {
            protocol_version: PROTOCOL_VERSION,
            request_id: format!(
                "{}-{}",
                std::process::id(),
                crate::feedback::unix_timestamp_ms()
            ),
            instance_id: String::new(),
            op,
            target,
            args,
            preconditions,
            timeout_ms,
        },
        instance,
        json_output,
    ))
}

fn parse_timeout(value: &str) -> Result<u64, String> {
    let millis = if let Some(seconds) = value.strip_suffix('s').filter(|_| !value.ends_with("ms")) {
        seconds
            .parse::<u64>()
            .ok()
            .and_then(|s| s.checked_mul(1000))
    } else {
        value.strip_suffix("ms").unwrap_or(value).parse().ok()
    };
    millis
        .filter(|ms| (1..=60000).contains(ms))
        .ok_or("timeout must be 1..60000 ms".into())
}

pub fn request(descriptor: &Descriptor, op: &str) -> Request {
    Request {
        protocol_version: PROTOCOL_VERSION,
        request_id: "discovery".into(),
        instance_id: descriptor.instance_id.clone(),
        op: op.into(),
        target: Target::default(),
        args: BTreeMap::new(),
        preconditions: Preconditions::default(),
        timeout_ms: 1000,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_protocol_arguments_and_encoding() {
        let (mut r, _, _) = parse_cli(vec![
            "pane".into(),
            "read".into(),
            "--pane".into(),
            "p7".into(),
            "--tail".into(),
            "100".into(),
            "--timeout".into(),
            "30s".into(),
        ])
        .unwrap();
        assert_eq!(r.timeout_ms, 30000);
        assert_eq!(r.target.pane_id.as_deref(), Some("p7"));
        r.instance_id = "test".into();
        validate(&r, "test").unwrap();
        r.args.insert("tail".into(), json!("100"));
        assert_eq!(validate(&r, "test").unwrap_err().code, "invalid_argument");
        r.args.insert("tail".into(), json!(100));
        r.preconditions.layout_token = Some("layout:old".into());
        assert_eq!(validate(&r, "test").unwrap_err().code, "invalid_argument");
        r.preconditions.layout_token = None;
        let capabilities = capabilities();
        for operation in capabilities["operations"].as_array().unwrap() {
            assert_eq!(operation["argument_schema"]["type"], "object");
            assert!(operation["result_schema"]["type"].is_string());
        }
        r.args.insert("typo".into(), json!(true));
        assert_eq!(validate(&r, "test").unwrap_err().code, "invalid_argument");
        assert!(serde_json::from_value::<Request>(json!({"unexpected":1})).is_err());
        assert!(parse_timeout("60001ms").is_err());
        let (exec, _, json_output) = parse_cli(
            [
                "tab",
                "new",
                "--exec",
                "tool.exe",
                "--",
                "--help",
                "--json",
                "two words",
            ]
            .map(str::to_string)
            .to_vec(),
        )
        .unwrap();
        assert_eq!(exec.args["argv"], json!(["--help", "--json", "two words"]));
        assert!(!json_output);
    }
}
