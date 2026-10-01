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
    for name in request.args.keys() {
        if !arguments.contains(&name.as_str()) {
            return Err(ControlError::new(
                "invalid_argument",
                format!("unknown argument {name}"),
            ));
        }
    }
    Ok(())
}

pub fn capabilities() -> Value {
    json!({"protocol_version": PROTOCOL_VERSION, "operations": OPERATIONS.iter().map(|(op,args)|
        json!({"op":op,"arguments":args})).collect::<Vec<_>>(), "platform":"windows",
        "limits":{"frame_bytes":MAX_FRAME_BYTES,"input_bytes":MAX_INPUT_BYTES,
            "read_rows":MAX_READ_ROWS,"connections":MAX_CONNECTIONS,"request_queue":32,"timeout_ms":60000}})
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
    match run_cli(arguments) {
        Ok(value) => {
            let ok = value["ok"].as_bool().unwrap_or(true);
            println!("{}", serde_json::to_string(&value).expect("JSON value"));
            if ok { 0 } else { 1 }
        }
        Err(error) => {
            println!(
                "{}",
                json!({"protocol_version":PROTOCOL_VERSION,"request_id":null,
                "instance_id":null,"revision":null,"ok":false,
                "error":{"code":"client_error","effect":"none","message":error}})
            );
            1
        }
    }
}

fn run_cli(arguments: Vec<String>) -> Result<Value, String> {
    if arguments.is_empty() || arguments.iter().any(|a| a == "--help" || a == "-h") {
        return Ok(
            json!({"usage":"mightty ctl OPERATION [--instance ID] [--window ID] [--tab ID] [--pane ID] [--json]",
            "operations":OPERATIONS.iter().map(|(op,args)|json!({"op":op,"arguments":args})).collect::<Vec<_>>() }),
        );
    }
    let (mut request, instance, json_output) = parse_cli(arguments)?;
    #[cfg(windows)]
    {
        let descriptors =
            crate::application::windows::discover_control_instances().map_err(|e| e.to_string())?;
        if request.op == "instances" {
            return Ok(json!({"protocol_version":PROTOCOL_VERSION,"ok":true,"result":descriptors}));
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
        let value = crate::application::windows::send_control(descriptor, &request)
            .map_err(|e| format!("outcome_unknown: {e}"))?;
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
        if matches!(option.as_str(), "--viewport" | "--focus") {
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
                    "pane.send-text" => {
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
            "cwd" => {
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
            "ratio" | "delta_px" | "rows" | "width" | "height" => {
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
        && op != "state"
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
    let instance = instance.or(inherited_instance);
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
        r.args.insert("typo".into(), json!(true));
        assert_eq!(validate(&r, "test").unwrap_err().code, "invalid_argument");
        assert!(serde_json::from_value::<Request>(json!({"unexpected":1})).is_err());
        assert!(parse_timeout("60001ms").is_err());
    }
}
