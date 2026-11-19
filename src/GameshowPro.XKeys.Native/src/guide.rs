// Copyright (c) Barjonas LLC 2026.
//! The logic behind `xkprobe guide`, the guided manual test. Its measured results are in `docs/device-measurements.md`.
//!
//! Everything here is pure or runs against the [`Hub`] and [`Input`] traits, so it can be unit-tested with recorded reports and
//! run without a person against a simulated pair of devices ([`SimHub`]). The real hub (the one that opens devices through the
//! `xkhid_*` exports) and the console input live in `examples/xkprobe.rs`.
//!
//! What it does: [`plan_steps`] turns the attached devices into an ordered list of [`Step`]s (press each key, turn the jog,
//! sweep the shuttle, close each input, press two keys at once, unplug and replug). [`run_session`] shows one plain-language prompt
//! per step, watches the devices' reports, shows what changed, decides pass or fail, lets the person skip or repeat, and never
//! waits without saying what for. [`result_json`] turns the outcome into the result file: every prompt, every raw report, the
//! derived key and input maps, and pass or fail per step.
//!
//! Only the Generate Data request (command 177) is ever sent to a device, through [`Hub::request_report`].

use crate::report::*;
use std::collections::{BTreeMap, BTreeSet};
use crate::lights::{self, LightModel, LightState};
use std::io::Write;

// ---------------------------------------------------------------------------------------------------------------------------
// JSON (dependency-free writer)
// ---------------------------------------------------------------------------------------------------------------------------

/// A JSON value, built in code and rendered by [`Json::render`].
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl From<&str> for Json {
    fn from(v: &str) -> Json {
        Json::Str(v.to_string())
    }
}
impl From<String> for Json {
    fn from(v: String) -> Json {
        Json::Str(v)
    }
}
impl From<bool> for Json {
    fn from(v: bool) -> Json {
        Json::Bool(v)
    }
}
impl From<i64> for Json {
    fn from(v: i64) -> Json {
        Json::Int(v)
    }
}
impl From<i32> for Json {
    fn from(v: i32) -> Json {
        Json::Int(i64::from(v))
    }
}
impl From<u32> for Json {
    fn from(v: u32) -> Json {
        Json::Int(i64::from(v))
    }
}
impl From<u16> for Json {
    fn from(v: u16) -> Json {
        Json::Int(i64::from(v))
    }
}
impl From<u8> for Json {
    fn from(v: u8) -> Json {
        Json::Int(i64::from(v))
    }
}
impl From<usize> for Json {
    fn from(v: usize) -> Json {
        Json::Int(v as i64)
    }
}
impl From<f64> for Json {
    fn from(v: f64) -> Json {
        Json::Num(v)
    }
}
impl<T: Into<Json>> From<Vec<T>> for Json {
    fn from(v: Vec<T>) -> Json {
        Json::Arr(v.into_iter().map(Into::into).collect())
    }
}
impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(v: Option<T>) -> Json {
        v.map_or(Json::Null, Into::into)
    }
}

impl Json {
    pub fn object() -> Json {
        Json::Obj(Vec::new())
    }

    /// Append a field to an object (no effect on other values).
    pub fn with(mut self, key: &str, value: impl Into<Json>) -> Json {
        if let Json::Obj(fields) = &mut self {
            fields.push((key.to_string(), value.into()));
        }
        self
    }

    fn is_scalar(&self) -> bool {
        !matches!(self, Json::Arr(_) | Json::Obj(_))
    }

    /// An object whose values are scalars or arrays of scalars: written on one line when it is an array element.
    fn is_flat(&self) -> bool {
        match self {
            Json::Obj(fields) => fields.iter().all(|(_, v)| v.is_scalar() || matches!(v, Json::Arr(a) if a.iter().all(Json::is_scalar))),
            _ => false,
        }
    }

    /// Pretty-printed text with a trailing newline. Flat objects inside arrays (such as one raw report) take one line each.
    pub fn render(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, 0);
        s.push('\n');
        s
    }

    fn write(&self, s: &mut String, indent: usize) {
        let pad = |s: &mut String, n: usize| s.push_str(&"  ".repeat(n));
        match self {
            Json::Arr(items) if items.iter().all(Json::is_scalar) => self.compact(s),
            Json::Arr(items) => {
                s.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    pad(s, indent + 1);
                    if item.is_flat() {
                        item.compact(s);
                    } else {
                        item.write(s, indent + 1);
                    }
                    s.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
                }
                pad(s, indent);
                s.push(']');
            }
            Json::Obj(fields) if fields.is_empty() => s.push_str("{}"),
            Json::Obj(fields) => {
                s.push_str("{\n");
                for (i, (k, v)) in fields.iter().enumerate() {
                    pad(s, indent + 1);
                    write_string(s, k);
                    s.push_str(": ");
                    v.write(s, indent + 1);
                    s.push_str(if i + 1 < fields.len() { ",\n" } else { "\n" });
                }
                pad(s, indent);
                s.push('}');
            }
            _ => self.compact(s),
        }
    }

    fn compact(&self, s: &mut String) {
        match self {
            Json::Null => s.push_str("null"),
            Json::Bool(b) => s.push_str(if *b { "true" } else { "false" }),
            Json::Int(i) => s.push_str(&i.to_string()),
            Json::Num(f) if f.is_finite() => s.push_str(&((f * 1000.0).round() / 1000.0).to_string()),
            Json::Num(_) => s.push_str("null"),
            Json::Str(t) => write_string(s, t),
            Json::Arr(items) => {
                s.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    item.compact(s);
                }
                s.push(']');
            }
            Json::Obj(fields) => {
                s.push('{');
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    write_string(s, k);
                    s.push_str(": ");
                    v.compact(s);
                }
                s.push('}');
            }
        }
    }
}

fn write_string(s: &mut String, text: &str) {
    s.push('"');
    for c in text.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            c if (c as u32) < 0x20 => s.push_str(&format!("\\u{:04x}", c as u32)),
            c => s.push(c),
        }
    }
    s.push('"');
}

// ---------------------------------------------------------------------------------------------------------------------------
// Dates
// ---------------------------------------------------------------------------------------------------------------------------

/// (year, month, day, hour, minute, second) in UTC for seconds since 1970-01-01.
pub fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400) as u32;
    // Howard Hinnant's days-to-civil algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// `2026-11-16T12:34:56Z`.
pub fn iso_utc(secs: i64) -> String {
    let (y, mo, d, h, mi, s) = civil_from_unix(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// `20261116T123456Z`, for file names.
pub fn file_stamp(secs: i64) -> String {
    let (y, mo, d, h, mi, s) = civil_from_unix(secs);
    format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}Z")
}

/// The result file's name: `xkprobe-guide-<UTC time>.json`.
pub fn result_file_name(secs: i64) -> String {
    format!("xkprobe-guide-{}.json", file_stamp(secs))
}

// ---------------------------------------------------------------------------------------------------------------------------
// Models and reports
// ---------------------------------------------------------------------------------------------------------------------------

/// The device families this tool knows how to prompt for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    JogShuttle,
    SwitchInterface12,
    SwitchInterface3,
    Unknown,
}

pub fn model_for_pid(pid: u16) -> Model {
    match pid {
        0x0426..=0x0428 => Model::JogShuttle,
        0x04A8..=0x04AB => Model::SwitchInterface12,
        0x04C5..=0x04C8 => Model::SwitchInterface3,
        _ => Model::Unknown,
    }
}

impl Model {
    pub fn name(self) -> &'static str {
        match self {
            Model::JogShuttle => "XK-12 Jog & Shuttle",
            Model::SwitchInterface12 => "XK-12 Switch Interface",
            Model::SwitchInterface3 => "XK-3 Switch Interface",
            Model::Unknown => "X-keys device",
        }
    }

    /// The USB polling interval measured on a test machine (`docs/device-measurements.md`, "Measured with `xkprobe probe`"): 8 ms for the Jog & Shuttle, 1 ms for the XK-12 Switch Interface.
    pub fn polling_interval_us(self) -> Option<i64> {
        match self {
            Model::JogShuttle => Some(8000),
            Model::SwitchInterface12 => Some(1000),
            _ => None,
        }
    }

    /// Input numbers (`(byte index - 3) * 8 + bit`) the P.I. Engineering documentation assigns to this model's keys or inputs.
    pub fn documented_inputs(self) -> &'static [u32] {
        match self {
            Model::JogShuttle => &[0, 1, 2, 8, 9, 10, 16, 17, 18, 24, 25, 26],
            Model::SwitchInterface12 => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            Model::SwitchInterface3 => &[0, 1, 4],
            Model::Unknown => &[],
        }
    }

    fn rank(self) -> u8 {
        match self {
            Model::JogShuttle => 0,
            Model::SwitchInterface12 => 1,
            Model::SwitchInterface3 => 2,
            Model::Unknown => 3,
        }
    }
}

/// The input numbers the consumers' current `Matrix(columns: 3, rows: 4)` table gives the Jog & Shuttle (0-3, 8-11, 16-19).
pub const CURRENT_MATRIX_3X4: [u32; 12] = [0, 1, 2, 3, 8, 9, 10, 11, 16, 17, 18, 19];

/// `(byte index - 3) * 8 + bit`: the consumers' input number for a bit of a report (the buttons start at byte 3).
pub fn input_number(index: usize, bit: u8) -> Option<u32> {
    index.checked_sub(3).map(|b| b as u32 * 8 + u32::from(bit))
}

/// One attached device as the guide sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceDesc {
    pub pid: u16,
    pub product: String,
    pub version: u16,
    pub input_len: u16,
    pub output_len: u16,
    pub timestamp_offset: i32,
    pub path: String,
}

impl DeviceDesc {
    pub fn model(&self) -> Model {
        model_for_pid(self.pid)
    }

    /// `XK-12 Jog & Shuttle (PID 0426)`.
    pub fn label(&self) -> String {
        let name = match self.model() {
            Model::Unknown if !self.product.is_empty() => self.product.clone(),
            m => m.name().to_string(),
        };
        format!("{name} (PID {:04X})", self.pid)
    }
}

/// Labels for a device list; two devices that would read the same get `, first` / `, second`... appended.
pub fn device_labels(devices: &[DeviceDesc]) -> Vec<String> {
    let base: Vec<String> = devices.iter().map(DeviceDesc::label).collect();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    base.iter()
        .map(|l| {
            let total = base.iter().filter(|x| *x == l).count();
            let n = seen.entry(l.clone()).or_insert(0);
            *n += 1;
            if total > 1 { format!("{l}, unit {n} of {total}") } else { l.clone() }
        })
        .collect()
}

/// Which bits of each report byte the guide compares: not the report ID or unit ID (bytes 0 and 1), not the "generated by Generate
/// Data" bit of the data type byte (byte 2), not the device timestamp, and not the Switch Interface's reboot counter (last byte).
pub fn comparison_mask(model: Model, len: usize, timestamp_offset: i32) -> Vec<u8> {
    let mut mask = vec![0xFFu8; len];
    for m in mask.iter_mut().take(2) {
        *m = 0;
    }
    if len > 2 {
        mask[2] = 0xFD;
    }
    if timestamp_offset >= 0 {
        for m in mask.iter_mut().skip(timestamp_offset as usize).take(4) {
            *m = 0;
        }
    }
    if matches!(model, Model::SwitchInterface12 | Model::SwitchInterface3) && len == 37 {
        mask[36] = 0;
    }
    mask
}

/// Reports whose data type byte (byte 2) is below 200 describe the controls' state; 214 is the descriptor report and 224 is custom
/// data, which the guide ignores.
pub fn is_state_report(data: &[u8]) -> bool {
    data.len() > 2 && data[2] < 200
}

/// One bit that differs between two reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BitChange {
    /// Byte index in the report; 0 is the report ID.
    pub index: usize,
    pub bit: u8,
    /// The bit's value after the change.
    pub set: bool,
}

/// The compared bits that differ between `before` and `after`, in byte then bit order. Bytes beyond `mask` are compared fully.
pub fn diff_bits(mask: &[u8], before: &[u8], after: &[u8]) -> Vec<BitChange> {
    let mut changes = Vec::new();
    for i in 0..before.len().min(after.len()) {
        let x = (before[i] ^ after[i]) & mask.get(i).copied().unwrap_or(0xFF);
        for bit in 0..8u8 {
            if x >> bit & 1 == 1 {
                changes.push(BitChange { index: i, bit, set: after[i] >> bit & 1 == 1 });
            }
        }
    }
    changes
}

/// The bytes with at least one compared bit different: `(index, before, after)`.
pub fn diff_bytes(mask: &[u8], before: &[u8], after: &[u8]) -> Vec<(usize, u8, u8)> {
    (0..before.len().min(after.len()))
        .filter(|&i| (before[i] ^ after[i]) & mask.get(i).copied().unwrap_or(0xFF) != 0)
        .map(|i| (i, before[i], after[i]))
        .collect()
}

/// `byte 3 bit 0 went ON, byte 4 bit 7 went OFF`.
pub fn describe_bits(changes: &[BitChange]) -> String {
    changes
        .iter()
        .map(|c| format!("byte {} bit {} went {}", c.index, c.bit, if c.set { "ON" } else { "OFF" }))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

fn bits_json(bits: &[BitChange]) -> Json {
    Json::Arr(
        bits.iter()
            .map(|b| Json::object().with("byte", b.index).with("bit", b.bit).with("went_on", b.set).with("input", input_number(b.index, b.bit)))
            .collect(),
    )
}

/// Follows one device's reports during a step against the state the step started from.
#[derive(Clone, Debug)]
pub struct Tracker {
    baseline: Vec<u8>,
    mask: Vec<u8>,
    /// Every bit that differed from the baseline at some point, with the value it first took.
    pub peak: BTreeMap<(usize, u8), bool>,
    /// For each byte that differed from the baseline: every value it took.
    pub bytes: BTreeMap<usize, BTreeSet<u8>>,
    /// Something differed from the baseline at some point.
    pub seen_active: bool,
    active: bool,
}

impl Tracker {
    pub fn new(baseline: Vec<u8>, mask: Vec<u8>) -> Tracker {
        Tracker { baseline, mask, peak: BTreeMap::new(), bytes: BTreeMap::new(), seen_active: false, active: false }
    }

    /// Take one report; returns the bits that differ from the baseline now (empty = back at rest).
    pub fn feed(&mut self, data: &[u8]) -> Vec<BitChange> {
        let now = diff_bits(&self.mask, &self.baseline, data);
        for c in &now {
            self.peak.entry((c.index, c.bit)).or_insert(c.set);
        }
        for (i, _, after) in diff_bytes(&self.mask, &self.baseline, data) {
            self.bytes.entry(i).or_default().insert(after);
        }
        self.active = !now.is_empty();
        self.seen_active |= self.active;
        now
    }

    /// Whether the last report differed from the baseline.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Every bit that differed at some point, as changes.
    pub fn bits(&self) -> Vec<BitChange> {
        self.peak.iter().map(|(&(index, bit), &set)| BitChange { index, bit, set }).collect()
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// Maps
// ---------------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapStatus {
    Mapped,
    NotTested,
    NoResponse,
    Ambiguous,
    Duplicate,
}

impl MapStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            MapStatus::Mapped => "mapped",
            MapStatus::NotTested => "not tested",
            MapStatus::NoResponse => "no response",
            MapStatus::Ambiguous => "ambiguous",
            MapStatus::Duplicate => "duplicate",
        }
    }
}

/// What one prompted key or input turned out to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapRow {
    pub label: String,
    pub status: MapStatus,
    pub index: Option<usize>,
    pub bit: Option<u8>,
    pub set: Option<bool>,
    /// `(byte index - 3) * 8 + bit`.
    pub input: Option<u32>,
    pub note: String,
}

/// Derive a map from `(label, bits that differed while the person pressed it)` pairs; `None` means the step was not run. A key
/// is mapped when exactly one bit changed; two keys that changed the same bit are flagged (the later one is the duplicate).
pub fn derive_map(responses: &[(String, Option<Vec<BitChange>>)]) -> Vec<MapRow> {
    let mut rows: Vec<MapRow> = responses
        .iter()
        .map(|(label, bits)| {
            let blank = |status, note: &str| MapRow { label: label.clone(), status, index: None, bit: None, set: None, input: None, note: note.to_string() };
            match bits {
                None => blank(MapStatus::NotTested, "skipped or not run"),
                Some(b) if b.is_empty() => blank(MapStatus::NoResponse, "nothing changed in any report"),
                Some(b) if b.len() > 1 => blank(MapStatus::Ambiguous, &format!("several bits changed: {}", describe_bits(b))),
                Some(b) => MapRow {
                    label: label.clone(),
                    status: MapStatus::Mapped,
                    index: Some(b[0].index),
                    bit: Some(b[0].bit),
                    set: Some(b[0].set),
                    input: input_number(b[0].index, b[0].bit),
                    note: String::new(),
                },
            }
        })
        .collect();
    let mut first: BTreeMap<(usize, u8), String> = BTreeMap::new();
    for row in &mut rows {
        if row.status != MapStatus::Mapped {
            continue;
        }
        let key = (row.index.unwrap(), row.bit.unwrap());
        match first.get(&key) {
            Some(other) => {
                row.status = MapStatus::Duplicate;
                row.note = format!("the same bit as {other}");
            }
            None => {
                first.insert(key, row.label.clone());
            }
        }
    }
    rows
}

// ---------------------------------------------------------------------------------------------------------------------------
// Steps
// ---------------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Keys,
    Jog,
    Shuttle,
    Inputs,
    Timing,
    Lights,
    Hotplug,
}

impl Group {
    pub fn name(self) -> &'static str {
        match self {
            Group::Keys => "keys",
            Group::Jog => "jog",
            Group::Shuttle => "shuttle",
            Group::Inputs => "inputs",
            Group::Timing => "timing",
            Group::Lights => "lights",
            Group::Hotplug => "hotplug",
        }
    }
}

/// Which parts of the session to run (`--only keys,jog,inputs,timing,lights,hotplug`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub keys: bool,
    pub jog: bool,
    pub inputs: bool,
    pub timing: bool,
    pub lights: bool,
    pub hotplug: bool,
    /// Test a Switch Interface contact by contact (for a TRS breakout) instead of socket by socket with one push button.
    pub per_contact: bool,
}

impl Selection {
    pub fn all() -> Selection {
        Selection { keys: true, jog: true, inputs: true, timing: true, lights: true, hotplug: true, per_contact: false }
    }

    pub fn parse(text: &str) -> Result<Selection, String> {
        let none = Selection { keys: false, jog: false, inputs: false, timing: false, lights: false, hotplug: false, per_contact: false };
        let mut s = none;
        for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match part {
                "keys" => s.keys = true,
                "jog" | "shuttle" => s.jog = true,
                "inputs" => s.inputs = true,
                "timing" => s.timing = true,
                "lights" => s.lights = true,
                "hotplug" => s.hotplug = true,
                "per-contact" => s.per_contact = true,
                "all" => s = Selection { per_contact: s.per_contact, ..Selection::all() },
                other => return Err(format!("unknown part \"{other}\"; use keys, jog, inputs, timing, lights, hotplug, per-contact or all")),
            }
        }
        if (Selection { per_contact: false, ..s }) == none {
            return Err("nothing selected".to_string());
        }
        Ok(s)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepKind {
    /// Press and release one key, or close and open one input. `slot` counts from 0 in prompt order.
    Press { slot: usize, label: String, total: usize },
    Jog { clockwise: bool },
    Shuttle { clockwise: bool },
    /// Two controls at the same moment, `reps` times; on one device (`partner` None) or one on each of two devices.
    /// With `plug_socket`, the person first plugs the push button into that socket (0-based) of the Switch Interface and leaves it.
    Pair { reps: u32, slots: (usize, usize), plug_socket: Option<usize> },
    /// One socket of the Switch Interface, tested with a single push button on a mono plug: plug in, press and hold, let go,
    /// unplug. `socket` is 0-based.
    Socket { socket: usize, total: usize },
    /// Record whatever the person does with a device the guide has no prompts for.
    Observe,
    /// Unplug the device, then plug it back in.
    Hotplug,
    /// A light (or a group of lights): for each phase the commands are sent, a question is asked, and the answer is judged.
    Light { phases: Vec<LightPhase> },
    /// Put every light this tool touched on the device back off.
    LightsCleanup,
}

// ---------------------------------------------------------------------------------------------------------------------------
// Lights (the `lights` part): what to switch, what to ask, how to judge the answer
// ---------------------------------------------------------------------------------------------------------------------------

/// One output report the lights part sends, built by [`crate::lights`] and checked by [`lights::validate_light_report`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightCommand {
    pub label: String,
    /// The full 36-byte report, report ID first.
    pub bytes: Vec<u8>,
}

fn light_cmd(label: impl Into<String>, bytes: [u8; lights::OUTPUT_REPORT_LEN]) -> LightCommand {
    LightCommand { label: label.into(), bytes: bytes.to_vec() }
}

/// How the person answers a question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// `y` or `n`.
    YesNo,
    /// Key numbers 1 to 12 (or `none`); compared with the expected set.
    Keys,
    /// Key numbers 1 to 12 (or `none`); only recorded.
    Observe,
    /// `y` to go on with this light, `n` to skip it (for the Switch Interface's electrical outputs).
    Gate,
}

/// What a person who sees correctly would observe after a phase's commands; simulations answer from it, and key questions are judged
/// against it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expect {
    Led { led: u8, state: LightState },
    Leds { green: bool, red: bool },
    FlashFaster,
    /// Exactly these keys (numbers 1 to 12) are lit.
    KeysLit(BTreeSet<u8>),
    AllBacklights { on: bool },
    Dimmer { bank: u8 },
    /// The backlights are switched off by the toggle (`off`) or back on.
    ToggleState { off: bool },
    /// Only this key (number 1 to 12) is lit, and flashing, in this bank.
    KeyFlashing { bank: u8, key: u8 },
    /// The "rows" command 182 was used on this bank with this mask: only recorded.
    Rows { bank: u8, mask: u8 },
    Output { out: u8, on: bool },
    Nothing,
}

/// One question: the commands sent first, the text shown, how it is answered, and what a correct observation is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightPhase {
    pub text: String,
    pub sends: Vec<LightCommand>,
    pub ask: Ask,
    pub expect: Expect,
}

/// A parsed answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    Keys(BTreeSet<u8>),
}

/// Parse what the person typed for a question; `Err` is the message to show before asking again.
pub fn parse_answer(ask: Ask, text: &str) -> Result<Answer, String> {
    let t = text.trim().to_ascii_lowercase();
    match ask {
        Ask::YesNo | Ask::Gate => match t.as_str() {
            "y" | "yes" => Ok(Answer::Yes),
            "n" | "no" => Ok(Answer::No),
            _ => Err("type  y  or  n  and press Enter".to_string()),
        },
        Ask::Keys | Ask::Observe => {
            if matches!(t.as_str(), "none" | "0" | "nothing") {
                return Ok(Answer::Keys(BTreeSet::new()));
            }
            let mut keys = BTreeSet::new();
            for token in t.split(|c: char| c.is_whitespace() || c == ',').filter(|s| !s.is_empty()) {
                match token.parse::<u8>() {
                    Ok(n) if (1..=12).contains(&n) => {
                        keys.insert(n);
                    }
                    _ => return Err(format!("\"{token}\" is not a key number from 1 to 12; type the numbers of the lit keys separated by spaces, or  none")),
                }
            }
            if keys.is_empty() { Err("type the numbers of the lit keys separated by spaces, or  none".to_string()) } else { Ok(Answer::Keys(keys)) }
        }
    }
}

/// `("pass" | "fail" | "recorded", detail)` for an answer to a question.
pub fn judge_light_answer(ask: Ask, expect: &Expect, answer: &Answer) -> (&'static str, String) {
    let list = |s: &BTreeSet<u8>| if s.is_empty() { "none".to_string() } else { s.iter().map(|k| k.to_string()).collect::<Vec<_>>().join(" ") };
    match (ask, answer) {
        (Ask::YesNo | Ask::Gate, Answer::Yes) => ("pass", String::new()),
        (Ask::YesNo | Ask::Gate, Answer::No) => ("fail", "the person answered no".to_string()),
        (Ask::Keys, Answer::Keys(seen)) => match expect {
            Expect::KeysLit(want) if want == seen => ("pass", format!("keys lit: {}", list(seen))),
            Expect::KeysLit(want) => ("fail", format!("keys lit: {}; the backlights switched on belong to keys {}", list(seen), list(want))),
            _ => ("recorded", format!("keys lit: {}", list(seen))),
        },
        (Ask::Observe, Answer::Keys(seen)) => ("recorded", format!("keys lit: {}", list(seen))),
        _ => ("fail", "the answer does not fit the question".to_string()),
    }
}

const FLASH_SLOW: u8 = 40;
const FLASH_FAST: u8 = 8;

fn key_numbers(first: u8) -> BTreeSet<u8> {
    (first..first + 4).collect()
}

/// A plain description of a Jog & Shuttle key for a prompt: its number and row.
const KEY_NUMBERING: &str = "Keys are numbered 1 to 12 along each row from the left, top row first (as in the key steps).";

fn led_phases(place: &str, led: u8) -> Vec<LightPhase> {
    let led_name = if led == lights::LED_GREEN { "green" } else { "red" };
    let cmd = |state: LightState| light_cmd(format!("{led_name} LED {state:?}"), lights::set_led(led, state).unwrap());
    let freq = |f: u8| light_cmd(format!("flash frequency {f}"), lights::set_flash_frequency(f).unwrap());
    let up = led_name.to_ascii_uppercase();
    vec![
        LightPhase {
            text: format!("The {up} indicator light ({place}) should now be ON and steady. Is it? (y/n)"),
            sends: vec![cmd(LightState::On)],
            ask: Ask::YesNo,
            expect: Expect::Led { led, state: LightState::On },
        },
        LightPhase {
            text: format!("The {up} light should now be FLASHING, slowly. Is it? (y/n)"),
            sends: vec![freq(FLASH_SLOW), cmd(LightState::Flash)],
            ask: Ask::YesNo,
            expect: Expect::Led { led, state: LightState::Flash },
        },
        LightPhase {
            text: format!("The {up} light should now flash FASTER than a moment ago. Does it? (y/n)"),
            sends: vec![freq(FLASH_FAST), cmd(LightState::Flash)],
            ask: Ask::YesNo,
            expect: Expect::FlashFaster,
        },
        LightPhase {
            text: format!("The {up} light should now be OFF. Is it? (y/n)"),
            sends: vec![cmd(LightState::Off)],
            ask: Ask::YesNo,
            expect: Expect::Led { led, state: LightState::Off },
        },
    ]
}

fn both_leds_phases(name: &str) -> Vec<LightPhase> {
    let c = |g: bool, r: bool| light_cmd(format!("both LEDs: green {}, red {}", if g { "on" } else { "off" }, if r { "on" } else { "off" }), lights::set_leds(g, r));
    vec![
        LightPhase { text: format!("Both the GREEN and the RED light on the {name} should now be ON. Are they? (y/n)"), sends: vec![c(true, true)], ask: Ask::YesNo, expect: Expect::Leds { green: true, red: true } },
        LightPhase { text: "Only the GREEN light should be on now, and the red one off. Is that right? (y/n)".to_string(), sends: vec![c(true, false)], ask: Ask::YesNo, expect: Expect::Leds { green: true, red: false } },
        LightPhase { text: "Only the RED light should be on now, and the green one off. Is that right? (y/n)".to_string(), sends: vec![c(false, true)], ask: Ask::YesNo, expect: Expect::Leds { green: false, red: true } },
        LightPhase { text: "Both lights should now be OFF. Are they? (y/n)".to_string(), sends: vec![c(false, false)], ask: Ask::YesNo, expect: Expect::Leds { green: false, red: false } },
    ]
}

fn clear_backlights() -> Vec<LightCommand> {
    (0..lights::BANKS).map(|b| light_cmd(format!("bank {} rows off", b + 1), lights::set_backlight_rows(b, 0).unwrap())).collect()
}

fn bank_key_phases(bank: u8) -> Vec<LightPhase> {
    (0..3u8)
        .map(|row| {
            let mut sends = clear_backlights();
            for col in 0..4u8 {
                let key_index = col * 8 + row;
                sends.push(light_cmd(format!("backlight bank {} key {} on", bank + 1, row * 4 + col + 1), lights::set_backlight(bank, key_index, LightState::On).unwrap()));
            }
            LightPhase {
                text: format!("I switched on some of the backlights (bank {}). Which keys are lit? Type their numbers separated by spaces, for example 1 2 3 4, or type none, then press Enter. {KEY_NUMBERING}", bank + 1),
                sends,
                ask: Ask::Keys,
                expect: Expect::KeysLit(key_numbers(row * 4 + 1)),
            }
        })
        .collect()
}

/// A light step before numbering: (id suffix, title, introduction, phases).
type LightStepPlan = (&'static str, String, String, Vec<LightPhase>);

/// The Jog & Shuttle's lights, in order, with the Switch Interface's below. Sources: see [`crate::lights`].
pub fn jog_shuttle_light_plan() -> Vec<LightStepPlan> {
    let name = "Jog & Shuttle";
    let place = "the small light at the top left of the case, above the keys";
    let all_on = || {
        (0..lights::BANKS).map(|b| light_cmd(format!("bank {} all rows on", b + 1), lights::set_backlight_rows(b, 0x3F).unwrap())).collect::<Vec<_>>()
    };
    let mut plan: Vec<LightStepPlan> = vec![
        ("green", "green indicator light".to_string(), format!("Look at the green light at the top left of the {name}. I will switch it on, make it flash slowly, flash faster, and switch it off, and ask what you see each time."), led_phases(place, lights::LED_GREEN)),
        ("red", "red indicator light".to_string(), format!("Look at the red light at the top left of the {name}. Same as before."), led_phases(place, lights::LED_RED)),
        ("both", "both indicator lights together".to_string(), "I will switch the green and red lights together with the \"set both\" command and ask what you see.".to_string(), both_leds_phases(name)),
        ("bank1-keys", "key backlights, bank 1".to_string(), "Each key has two backlights, called bank 1 and bank 2 in the documentation (they may be different colors). I will light bank 1 of one row at a time, key by key, and ask you which keys are lit.".to_string(), bank_key_phases(0)),
        ("bank2-keys", "key backlights, bank 2".to_string(), "Same for bank 2.".to_string(), bank_key_phases(1)),
    ];
    let rows_phase = |bank: u8| {
        let mut sends = clear_backlights();
        sends.push(light_cmd(format!("bank {} rows mask 0x01", bank + 1), lights::set_backlight_rows(bank, 0x01).unwrap()));
        LightPhase {
            text: format!("I used the \"rows\" command on bank {} (row 1 only). Which keys are lit? Type their numbers separated by spaces, or none. I only record this, because the documentation does not say which keys a row is. {KEY_NUMBERING}", bank + 1),
            sends,
            ask: Ask::Observe,
            expect: Expect::Rows { bank, mask: 0x01 },
        }
    };
    plan.push(("rows", "backlight rows command".to_string(), "The documentation has a command that switches \"rows\" of backlights. I will try it once on each bank and ask which keys light up; nothing is judged.".to_string(), vec![rows_phase(0), rows_phase(1)]));
    plan.push((
        "all",
        "all backlights on, then off".to_string(),
        "I will switch every backlight on, then every backlight off.".to_string(),
        vec![
            LightPhase { text: "Every key should now be lit (both backlights in each key). Is that right? (y/n)".to_string(), sends: all_on(), ask: Ask::YesNo, expect: Expect::AllBacklights { on: true } },
            LightPhase { text: "No key should be lit now. Is that right? (y/n)".to_string(), sends: clear_backlights(), ask: Ask::YesNo, expect: Expect::AllBacklights { on: false } },
        ],
    ));
    let mut full = all_on();
    full.push(light_cmd("intensity 255 / 255", lights::set_backlight_intensity(255, 255)));
    plan.push((
        "intensity",
        "backlight brightness".to_string(),
        "I will light every backlight at full brightness and then dim bank 1, then bank 2, and ask whether it got dimmer.".to_string(),
        vec![
            LightPhase { text: "Every key should be lit at full brightness. Is that right? (y/n)".to_string(), sends: full, ask: Ask::YesNo, expect: Expect::AllBacklights { on: true } },
            LightPhase { text: "I dimmed bank 1 (one of the two backlights in each key). Is the lighting dimmer than a moment ago? (y/n)".to_string(), sends: vec![light_cmd("intensity 30 / 255", lights::set_backlight_intensity(30, 255))], ask: Ask::YesNo, expect: Expect::Dimmer { bank: 0 } },
            LightPhase { text: "Bank 1 is back to full and I dimmed bank 2. Did the other backlight in each key get dimmer? (y/n)".to_string(), sends: vec![light_cmd("intensity 255 / 30", lights::set_backlight_intensity(255, 30))], ask: Ask::YesNo, expect: Expect::Dimmer { bank: 1 } },
        ],
    ));
    plan.push((
        "toggle",
        "toggle backlights".to_string(),
        "The documentation has a \"toggle backlights\" command (like the Scroll Lock key). I will use it twice.".to_string(),
        vec![
            LightPhase { text: "I toggled the backlights. Did they change (switch off, or on)? (y/n)".to_string(), sends: vec![light_cmd("toggle backlights", lights::toggle_backlights())], ask: Ask::YesNo, expect: Expect::ToggleState { off: true } },
            LightPhase { text: "I toggled them again. Are they back to how they were before the first toggle? (y/n)".to_string(), sends: vec![light_cmd("toggle backlights", lights::toggle_backlights())], ask: Ask::YesNo, expect: Expect::ToggleState { off: false } },
        ],
    ));
    let flash = |bank: u8| {
        let mut sends = clear_backlights();
        sends.push(light_cmd(format!("flash frequency {FLASH_SLOW}"), lights::set_flash_frequency(FLASH_SLOW).unwrap()));
        sends.push(light_cmd(format!("backlight bank {} key 1 flash", bank + 1), lights::set_backlight(bank, 0, LightState::Flash).unwrap()));
        LightPhase {
            text: format!("One backlight should be FLASHING: key 1 (top left key), bank {}, and no other. Is that what you see? (y/n)", bank + 1),
            sends,
            ask: Ask::YesNo,
            expect: Expect::KeyFlashing { bank, key: 1 },
        }
    };
    plan.push(("flash", "a flashing backlight".to_string(), "I will make the top left key's backlight flash, first bank 1, then bank 2.".to_string(), vec![flash(0), flash(1)]));
    plan
}

/// The Switch Interface's lights: indicator LEDs and the two electrical outputs (179, LED index 0 and 1).
pub fn switch_interface_light_plan(name: &str) -> Vec<LightStepPlan> {
    let place = format!("the small light on the case of the {name}");
    let out_phases = |out: u8| {
        let cmd = |on: bool| light_cmd(format!("OUT {} {}", out + 1, if on { "on" } else { "off" }), lights::set_led(out, if on { LightState::On } else { LightState::Off }).unwrap());
        vec![
            LightPhase { text: format!("The {name} has two electrical outputs, OUT 1 and OUT 2. Is something connected to OUT {} that shows whether it is on (an LED or a meter), and do you want to test it? (y/n)", out + 1), sends: vec![], ask: Ask::Gate, expect: Expect::Nothing },
            LightPhase { text: format!("OUT {} is now switched ON. Does what you connected show it on? (y/n)", out + 1), sends: vec![cmd(true)], ask: Ask::YesNo, expect: Expect::Output { out, on: true } },
            LightPhase { text: format!("OUT {} is now switched OFF. Does it show off? (y/n)", out + 1), sends: vec![cmd(false)], ask: Ask::YesNo, expect: Expect::Output { out, on: false } },
        ]
    };
    vec![
        ("green", "green indicator light".to_string(), format!("Look at the green light on the {name}. I will switch it on, make it flash slowly, flash faster, and switch it off, and ask what you see each time."), led_phases(&place, lights::LED_GREEN)),
        ("red", "red indicator light".to_string(), format!("Look at the red light on the {name}. Same as before."), led_phases(&place, lights::LED_RED)),
        ("out1", "electrical output OUT 1".to_string(), "Optional: only if you have something connected to OUT 1. Otherwise answer n to the first question.".to_string(), out_phases(lights::LED_OUT1)),
        ("out2", "electrical output OUT 2".to_string(), "Optional: only if you have something connected to OUT 2. Otherwise answer n to the first question.".to_string(), out_phases(lights::LED_OUT2)),
    ]
}

/// What puts the lights back: indicators off, backlights off, intensity at 255 / 255, the toggle undone if it was used an odd number
/// of times, and the electrical outputs off if they were switched.
pub fn cleanup_commands(model: Model, outputs_touched: &[u8], toggle_odd: bool) -> Vec<LightCommand> {
    let mut v = Vec::new();
    for (led, name) in [(lights::LED_GREEN, "green"), (lights::LED_RED, "red")] {
        v.push(light_cmd(format!("{name} LED off"), lights::set_led(led, LightState::Off).unwrap()));
    }
    for &out in outputs_touched {
        v.push(light_cmd(format!("OUT {} off", out + 1), lights::set_led(out, LightState::Off).unwrap()));
    }
    if model == Model::JogShuttle {
        if toggle_odd {
            v.push(light_cmd("toggle backlights (undo)", lights::toggle_backlights()));
        }
        for bank in 0..lights::BANKS {
            for key in lights::JOG_SHUTTLE_KEY_INDEXES {
                v.push(light_cmd(format!("backlight bank {} index {} off", bank + 1, key + 32 * bank), lights::set_backlight(bank, key, LightState::Off).unwrap()));
            }
        }
        v.extend(clear_backlights());
        v.push(light_cmd("intensity 255 / 255", lights::set_backlight_intensity(255, 255)));
    }
    v
}

pub(crate) fn light_model(model: Model) -> LightModel {
    if model == Model::JogShuttle { LightModel::JogShuttle } else { LightModel::SwitchInterface }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// 1-based position in the session.
    pub number: usize,
    /// Stable name, for the result file and for scripts (`js0-key-01`).
    pub id: String,
    pub group: Group,
    /// Index into the device list.
    pub device: usize,
    /// The second device of a cross-device timing step.
    pub partner: Option<usize>,
    pub device_label: String,
    pub title: String,
    /// What the person is asked to do, in plain words.
    pub prompt: String,
    pub kind: StepKind,
}

const SI12_NAMES: [&str; 12] = ["1R", "1L", "2R", "2L", "3R", "3L", "4R", "4L", "5R", "5L", "6R", "6L"];

fn slot_name(model: Model, slot: usize) -> String {
    match model {
        Model::JogShuttle => format!("key {}", slot + 1),
        Model::SwitchInterface12 => format!("input {}", SI12_NAMES[slot]),
        Model::SwitchInterface3 => format!("switch SW{}", slot + 1),
        Model::Unknown => format!("control {}", slot + 1),
    }
}

/// The documented bit for a prompt slot, or `None` for an unknown model or an out-of-range slot.
pub fn bit_for_slot(model: Model, slot: usize) -> Option<(usize, u8)> {
    match model {
        // Measured on a test machine: keys numbered row by row from the top left (4 keys per row, 3 rows) are byte 3 + (position in the row),
        // bit 0 for the top row, 1 for the middle row, 2 for the bottom row.
        Model::JogShuttle if slot < 12 => Some((3 + slot % 4, (slot / 4) as u8)),
        Model::SwitchInterface12 if slot < 12 => Some((3 + slot / 8, (slot % 8) as u8)),
        Model::SwitchInterface3 if slot < 3 => Some((3, [1u8, 0, 4][slot])),
        _ => None,
    }
}

fn control_count(model: Model) -> usize {
    match model {
        Model::JogShuttle | Model::SwitchInterface12 => 12,
        Model::SwitchInterface3 => 3,
        Model::Unknown => 0,
    }
}

fn press_prompt(model: Model, slot: usize, total: usize) -> String {
    match model {
        Model::JogShuttle => {
            let hint = match slot {
                0 => " (the top-left key)",
                s if s + 1 == total => " (the last key, bottom right)",
                _ => "",
            };
            format!(
                "Press key {} of {total}{hint}, then let go. Keys are counted along each row from the left, starting with the top row.",
                slot + 1
            )
        }
        Model::SwitchInterface12 => format!(
            "Close input {} of {total} ({}; jack {}, {} contact): connect it with a switch or a wire, then open it again.",
            slot + 1,
            SI12_NAMES[slot],
            slot / 2 + 1,
            if slot % 2 == 0 { "R" } else { "L" }
        ),
        _ => format!("Close switch SW{} of {total}, then open it again.", slot + 1),
    }
}

fn pair_prompt(model: Model, slots: (usize, usize), cross_with: Option<&str>) -> String {
    if let Some(other) = cross_with {
        return format!(
            "At the same moment, press one key (or close one input) on each of the two devices: this {} and the {other}. Then let go of both.",
            model.name()
        );
    }
    let (a, b) = (slot_name(model, slots.0), slot_name(model, slots.1));
    match model {
        Model::JogShuttle => format!("Press {a} and {b} at the very same moment (use two fingers), then let go of both."),
        _ => format!("Close {a} and {b} at the very same moment, then open both."),
    }
}

/// Said once before the first light question.
pub const LIGHTS_INTRO: &str = "\
The lights part: I will now switch the lights on the devices myself, one at a time, and ask what you see. Look at the device when I ask.
  Answer by typing  y  or  n  (or the key numbers where I ask for them) and pressing Enter; skip, repeat and quit work as before.
  I only use the temporary light commands: nothing is saved inside a device, and no setting, unit ID or mode is changed.
  When a device is done, I switch every light I touched back off.";

/// Sockets on the XK-12 Switch Interface.
pub const SOCKETS: usize = 6;

/// Detents of the jog dial in one direction that make the jog step pass.
pub const JOG_MIN_DETENTS: u32 = 3;

/// Said once, before the first socket of each Switch Interface.
pub const SOCKET_INTRO: &str = "\
For the Switch Interface you need ONE push button wired to a mono (tip and sleeve) jack plug. There are 6 sockets, and I test them one at a time.
  I count the sockets 1 to 6 from the LEFT, looking at the sockets. If you will count from the RIGHT instead (or the case is numbered the other
  way round), type  right  and press Enter now; I will record which end you counted from. (Type  left  to change back.)
  For each socket I ask you to plug the button in, press and hold it, let go, and unplug it, and I record what the report does each time.";

fn socket_prompt(socket: usize) -> String {
    format!(
        "Socket {} of {SOCKETS} (counting from the left, or from the end you told me). First make sure NOTHING is plugged into the Switch Interface; I read the empty state now. I will then ask you to plug the button into socket {}.",
        socket + 1,
        socket + 1
    )
}

/// Turn the attached devices into the ordered list of steps: for each device its keys or inputs (and jog and shuttle), then the
/// timing rounds, then hot-plug. Devices are ordered Jog & Shuttle first. `reps` is the number of rounds per timing step.
pub fn plan_steps(devices: &[DeviceDesc], sel: &Selection, reps: u32) -> Vec<Step> {
    let labels = device_labels(devices);
    let mut order: Vec<usize> = (0..devices.len()).collect();
    order.sort_by_key(|&i| devices[i].model().rank());
    let mut steps: Vec<Step> = Vec::new();
    let mut add = |id: String, group: Group, device: usize, partner: Option<usize>, title: String, prompt: String, kind: StepKind| {
        steps.push(Step {
            number: 0,
            id,
            group,
            device,
            partner,
            device_label: labels[device].clone(),
            title: format!("{} - {title}", labels[device]),
            prompt,
            kind,
        });
    };
    let tag = |d: usize| format!("{}{d}", match devices[d].model() {
        Model::JogShuttle => "js",
        Model::SwitchInterface12 => "si",
        Model::SwitchInterface3 => "s3",
        Model::Unknown => "xk",
    });
    for &d in &order {
        let model = devices[d].model();
        let total = control_count(model);
        match model {
            Model::JogShuttle => {
                if sel.keys {
                    for slot in 0..total {
                        let label = slot_name(model, slot);
                        add(
                            format!("{}-key-{:02}", tag(d), slot + 1),
                            Group::Keys,
                            d,
                            None,
                            label.clone(),
                            press_prompt(model, slot, total),
                            StepKind::Press { slot, label, total },
                        );
                    }
                }
                if sel.jog {
                    for (clockwise, word) in [(true, "clockwise"), (false, "counter-clockwise")] {
                        add(
                            format!("{}-jog-{}", tag(d), if clockwise { "cw" } else { "ccw" }),
                            Group::Jog,
                            d,
                            None,
                            format!("jog dial {word}"),
                            format!(
                                "Turn the jog dial (the free-spinning wheel) {word}, about {} clicks, always the same way, then stop and keep still for a moment.",
                                JOG_MIN_DETENTS + 2
                            ),
                            StepKind::Jog { clockwise },
                        );
                    }
                    for (clockwise, word) in [(true, "clockwise"), (false, "counter-clockwise")] {
                        add(
                            format!("{}-shuttle-{}", tag(d), if clockwise { "cw" } else { "ccw" }),
                            Group::Shuttle,
                            d,
                            None,
                            format!("shuttle ring {word}"),
                            format!(
                                "Turn the shuttle ring (the spring-loaded outer ring) {word} one notch at a time, pausing about a second at each notch, as far as it goes. Then let go so it springs back to the middle."
                            ),
                            StepKind::Shuttle { clockwise },
                        );
                    }
                }
            }
            Model::SwitchInterface12 if sel.inputs && !sel.per_contact => {
                for socket in 0..SOCKETS {
                    add(
                        format!("{}-socket-{}", tag(d), socket + 1),
                        Group::Inputs,
                        d,
                        None,
                        format!("socket {} of {SOCKETS}", socket + 1),
                        socket_prompt(socket),
                        StepKind::Socket { socket, total: SOCKETS },
                    );
                }
            }
            Model::SwitchInterface12 | Model::SwitchInterface3 => {
                if sel.inputs && (model == Model::SwitchInterface3 || sel.per_contact) {
                    for slot in 0..total {
                        let label = slot_name(model, slot);
                        let id_name = SI12_NAMES.get(slot).filter(|_| model == Model::SwitchInterface12).map_or_else(|| format!("sw{}", slot + 1), |n| n.to_string());
                        add(
                            format!("{}-input-{id_name}", tag(d)),
                            Group::Inputs,
                            d,
                            None,
                            label.clone(),
                            press_prompt(model, slot, total),
                            StepKind::Press { slot, label, total },
                        );
                    }
                }
            }
            Model::Unknown => {
                if sel.keys || sel.inputs || sel.jog {
                    add(
                        format!("{}-observe", tag(d)),
                        Group::Keys,
                        d,
                        None,
                        "everything it has".to_string(),
                        "This model has no prepared prompts. For about ten seconds, press every key and move every control on it; I will record whatever it sends.".to_string(),
                        StepKind::Observe,
                    );
                }
            }
        }
    }
    if sel.timing {
        let timed: Vec<usize> = order.iter().copied().filter(|&d| control_count(devices[d].model()) >= 3).collect();
        for &d in &timed {
            let model = devices[d].model();
            if model == Model::SwitchInterface12 && !sel.per_contact {
                continue; // one push button cannot make two inputs at once; see the cross-device step
            }
            let total = control_count(model);
            let slots = (0, if model == Model::SwitchInterface3 { 2 } else { total - 1 });
            add(
                format!("{}-timing", tag(d)),
                Group::Timing,
                d,
                None,
                format!("two {} at once", if model == Model::JogShuttle { "keys" } else { "inputs" }),
                pair_prompt(model, slots, None),
                StepKind::Pair { reps, slots, plug_socket: None },
            );
        }
        if timed.len() >= 2 {
            let (a, b) = (timed[0], timed[1]);
            let button = [a, b].iter().any(|&x| devices[x].model() == Model::SwitchInterface12) && !sel.per_contact;
            let (prompt, plug_socket) = if button {
                (
                    "First plug the push button into socket 1 of the Switch Interface and leave it there. Then, at the very same moment, press the button and any key on the Jog & Shuttle, and let go of both.".to_string(),
                    Some(0),
                )
            } else {
                (pair_prompt(devices[a].model(), (0, 0), Some(&labels[b])), None)
            };
            add(
                "cross-timing".to_string(),
                Group::Timing,
                a,
                Some(b),
                format!("one control on each of the two devices at once (with the {})", labels[b]),
                prompt,
                StepKind::Pair { reps, slots: (0, 0), plug_socket },
            );
        }
    }
    if sel.lights {
        for &d in &order {
            let model = devices[d].model();
            let plan = match model {
                Model::JogShuttle => jog_shuttle_light_plan(),
                Model::SwitchInterface12 | Model::SwitchInterface3 => switch_interface_light_plan(model.name()),
                Model::Unknown => continue,
            };
            for (suffix, title, intro, phases) in plan {
                add(format!("{}-light-{suffix}", tag(d)), Group::Lights, d, None, title, intro, StepKind::Light { phases });
            }
            add(
                format!("{}-lights-off", tag(d)),
                Group::Lights,
                d,
                None,
                "put every light back off".to_string(),
                "I switch every light I touched back off.".to_string(),
                StepKind::LightsCleanup,
            );
        }
    }
    if sel.hotplug {
        for &d in &order {
            add(
                format!("{}-hotplug", tag(d)),
                Group::Hotplug,
                d,
                None,
                "unplug and replug".to_string(),
                "Pull the USB cable of this device out, wait a moment, then plug it back in. I will tell you when.".to_string(),
                StepKind::Hotplug,
            );
        }
    }
    for (i, s) in steps.iter_mut().enumerate() {
        s.number = i + 1;
    }
    steps
}

// ---------------------------------------------------------------------------------------------------------------------------
// Timing analysis
// ---------------------------------------------------------------------------------------------------------------------------

/// The first report in which one control of a timing round showed up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirstSeen {
    pub device: usize,
    pub key: (usize, u8),
    pub t_us: i64,
    pub device_ms: Option<u32>,
    pub sequence: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoundStats {
    pub first: FirstSeen,
    pub second: FirstSeen,
    /// Both arrived in one report.
    pub same_report: bool,
    /// Host time between the two reports (0 for one report).
    pub gap_us: i64,
    /// The device's own millisecond difference, for two controls on one device.
    pub device_gap_ms: Option<i64>,
    /// For one device with a known polling interval: the gap is shorter than that interval.
    pub within_polling_interval: Option<bool>,
}

/// Work out one round from the first report each control appeared in. For two devices the earliest report of each is compared;
/// for one device the first two controls. `None` when too few controls were seen.
pub fn analyze_round(firsts: &[FirstSeen], cross: bool, polling_interval_us: Option<i64>) -> Option<RoundStats> {
    let mut sorted: Vec<&FirstSeen> = firsts.iter().collect();
    sorted.sort_by_key(|f| f.t_us);
    let (a, b) = if cross {
        let a = *sorted.first()?;
        let b = *sorted.iter().find(|f| f.device != a.device)?;
        (a, b)
    } else {
        (*sorted.first()?, *sorted.get(1)?)
    };
    let same_device = a.device == b.device;
    let same_report = same_device && a.sequence == b.sequence;
    let gap_us = if same_report { 0 } else { (b.t_us - a.t_us).abs() };
    let device_gap_ms = match (same_device, a.device_ms, b.device_ms) {
        (true, Some(x), Some(y)) => Some(i64::from(y.wrapping_sub(x) as i32).abs()),
        _ => None,
    };
    let within = if same_device { polling_interval_us.map(|p| same_report || gap_us < p) } else { None };
    Some(RoundStats { first: a.clone(), second: b.clone(), same_report, gap_us, device_gap_ms, within_polling_interval: within })
}

// ---------------------------------------------------------------------------------------------------------------------------
// Hub, input, options
// ---------------------------------------------------------------------------------------------------------------------------

/// What the person typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Go,
    Skip,
    Repeat,
    Quit,
    /// The person counts the Switch Interface's sockets from the left (the default) or from the right.
    Left,
    Right,
}

/// Words: `skip`, `repeat` (or `again`), `quit` (or `stop`), `left`, `right`, `go` (or just Enter). Anything else is not a command,
/// so stray characters typed by a key that also acts as a keyboard key do no harm.
pub fn parse_command(line: &str) -> Option<Command> {
    match line.trim().to_ascii_lowercase().as_str() {
        "left" => Some(Command::Left),
        "right" => Some(Command::Right),
        "" | "go" | "start" => Some(Command::Go),
        "skip" => Some(Command::Skip),
        "repeat" | "again" => Some(Command::Repeat),
        "quit" | "stop" => Some(Command::Quit),
        _ => None,
    }
}

/// Where commands come from: the console, or a script.
pub trait Input {
    /// A command typed since the last call, if any. `step` and `attempt` say what is being waited on (scripts key on them).
    fn poll(&mut self, step: &str, attempt: u32) -> Option<Command>;
    /// A line the person typed that is not a command (an answer such as `y` or `1 2 3 4`), if any.
    fn answer(&mut self, _step: &str, _attempt: u32) -> Option<String> {
        None
    }
    /// Forget anything typed so far that was not asked for (called before each question).
    fn discard_pending(&mut self) {}
}

/// Commands keyed to a step id and attempt number, for non-interactive runs.
#[derive(Default)]
pub struct ScriptedInput {
    commands: Vec<(String, u32, Command)>,
    answers: Vec<(String, u32, String)>,
}

impl ScriptedInput {
    pub fn new(commands: Vec<(String, u32, Command)>) -> ScriptedInput {
        ScriptedInput { commands, answers: Vec::new() }
    }

    /// Lines of `step-id [attempt] skip|repeat|quit|left|right`, or `step-id [#attempt] answer text` (for the light questions, one
    /// line per question, used in order). Old form: `step-id attempt command`.
    ///
    /// Original doc: Lines of `step-id attempt command`; blank lines and `#` comments are ignored. Attempt may be omitted (1).
    pub fn parse(text: &str) -> Result<ScriptedInput, String> {
        let mut commands = Vec::new();
        let mut answers = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            let id = parts[0].to_string();
            let rest = &parts[1..];
            let as_command = |w: &str| parse_command(w).filter(|c| *c != Command::Go);
            match rest {
                [] => return Err(format!("line {}: expected \"step-id [attempt] skip|repeat|quit\" or \"step-id answer\"", n + 1)),
                [word] if as_command(word).is_some() => commands.push((id, 1, as_command(word).unwrap())),
                [attempt, word] if attempt.parse::<u32>().is_ok() && as_command(word).is_some() => {
                    commands.push((id, attempt.parse().unwrap(), as_command(word).unwrap()))
                }
                [first, tail @ ..] if first.starts_with('#') && first[1..].parse::<u32>().is_ok() && !tail.is_empty() => {
                    answers.push((id, first[1..].parse().unwrap(), tail.join(" ")))
                }
                _ => answers.push((id, 1, rest.join(" "))),
            }
        }
        Ok(ScriptedInput { commands, answers })
    }
}

impl Input for ScriptedInput {
    fn poll(&mut self, step: &str, attempt: u32) -> Option<Command> {
        let at = self.commands.iter().position(|(s, a, _)| s == step && *a == attempt)?;
        Some(self.commands.remove(at).2)
    }

    fn answer(&mut self, step: &str, attempt: u32) -> Option<String> {
        let at = self.answers.iter().position(|(s, a, _)| s == step && *a == attempt)?;
        Some(self.answers.remove(at).2)
    }
}

/// The devices and the clock. The real implementation opens the devices through the `xkhid_*` exports; [`SimHub`] simulates them.
pub trait Hub {
    fn devices(&self) -> &[DeviceDesc];
    /// Microseconds since the session began, for a QPC tick count.
    fn to_us(&self, qpc: i64) -> i64;
    fn now_us(&self) -> i64;
    /// Wait up to `max_wait_us` and return the entries that arrived from any device, oldest first. May return sooner.
    fn poll(&mut self, max_wait_us: i64) -> Vec<(usize, ReportEntry)>;
    /// Send the Generate Data request (command 177); the only command ever sent.
    fn request_report(&mut self, device: usize) -> Result<(), String>;
    /// Called when a step (or one round or stage of it) begins; simulations use it to schedule their answers.
    fn begin_phase(&mut self, _step: &Step, _phase: u32) {}
    /// Whether the device's session is open (it closes when the device reports an error).
    fn is_open(&self, device: usize) -> bool;
    /// Open the device again after a replug.
    fn reopen(&mut self, device: usize) -> Result<(), String>;
    /// Arrivals plus removals counted by the device watch.
    fn watch_generation(&self) -> u64;
    /// Send one volatile light report (see [`crate::lights`]); the engine has already validated it. The real hub validates again.
    fn send_light(&mut self, _device: usize, _report: &[u8]) -> Result<(), String> {
        Err("this hub cannot send light commands".to_string())
    }
    /// For simulations: the answer a person with good eyes would give to a light question right now.
    fn simulated_answer(&mut self, _device: usize, _ask: Ask, _expect: &Expect) -> Option<String> {
        None
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    /// How long a step waits for the person.
    pub timeout_us: i64,
    /// Pause after a passed step, during which `repeat` is accepted.
    pub pause_pass_us: i64,
    /// Pause after a failed step.
    pub pause_fail_us: i64,
    /// The jog step ends when the dial has been quiet this long.
    pub quiet_us: i64,
    /// How often a waiting step says what it is still waiting for.
    pub reminder_us: i64,
    /// Wait for the person to say go before the first step.
    pub wait_for_go: bool,
    /// Do not ask a person for anything that needs hands (hot-plug is recorded as skipped).
    pub skip_hotplug: bool,
    /// How long the guide waits for the person to say go before giving up.
    pub go_timeout_us: i64,
}

impl Options {
    /// The defaults for a person at the devices: 30 s per step, a 3 s pause after a pass and 15 s after a failure.
    pub fn interactive() -> Options {
        Options {
            timeout_us: 30_000_000,
            pause_pass_us: 3_000_000,
            pause_fail_us: 15_000_000,
            quiet_us: 1_500_000,
            reminder_us: 10_000_000,
            wait_for_go: true,
            skip_hotplug: false,
            go_timeout_us: 300_000_000,
        }
    }

    /// No person: no start prompt, no pauses.
    pub fn unattended(timeout_us: i64) -> Options {
        Options { timeout_us, pause_pass_us: 0, pause_fail_us: 0, wait_for_go: false, ..Options::interactive() }
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail,
    Skipped,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Fail => "fail",
            Outcome::Skipped => "skipped",
        }
    }
}

/// Most raw reports kept per attempt in the result file.
pub const MAX_LOGGED_PER_ATTEMPT: usize = 3000;

/// One raw entry as recorded in the result file.
#[derive(Clone, Debug, PartialEq)]
pub struct LoggedReport {
    pub device: usize,
    pub sequence: u32,
    pub t_us: i64,
    pub device_ms: Option<u32>,
    pub flags: u32,
    /// The error code of a session-ending entry, else 0.
    pub error: i32,
    pub win32_error: u32,
    pub data: String,
    /// Sent because the guide asked (Generate Data), not by a key press.
    pub generated: bool,
}

impl LoggedReport {
    fn from_entry(device: usize, e: &ReportEntry, t_us: i64) -> LoggedReport {
        let data = &e.data[..(e.length as usize).min(MAX_REPORT_LEN)];
        LoggedReport {
            device,
            sequence: e.sequence,
            t_us,
            device_ms: (e.flags & FLAG_HAS_DEVICE_MS != 0).then_some(e.device_ms),
            flags: e.flags,
            error: if e.flags & FLAG_ERROR != 0 { e.error } else { 0 },
            win32_error: e.win32_error,
            data: hex(data),
            generated: data.get(2).is_some_and(|t| t & 0x02 != 0 && *t < 200),
        }
    }

    fn json(&self) -> Json {
        Json::object()
            .with("device", self.device)
            .with("sequence", self.sequence)
            .with("t_us", self.t_us)
            .with("device_ms", self.device_ms)
            .with("flags", self.flags)
            .with("generated", self.generated)
            .with("error", if self.flags & FLAG_ERROR != 0 { Some(self.error) } else { None })
            .with("win32_error", self.win32_error)
            .with("data", self.data.as_str())
    }
}

/// One go at a step.
#[derive(Clone, Debug)]
pub struct Attempt {
    pub outcome: Outcome,
    pub note: String,
    /// Step-specific details (changed bits, rounds, ...).
    pub response: Json,
    /// For a key or input: the bits that changed while it was pressed.
    pub bits: Vec<BitChange>,
    pub reports: Vec<LoggedReport>,
    pub reports_dropped: usize,
    /// For a Switch Interface socket step: what the four phases showed.
    pub socket: Option<SocketResult>,
    pub started_us: i64,
    pub ended_us: i64,
}

/// What plugging a push button into one Switch Interface socket, pressing it, letting go and unplugging it did to the report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SocketResult {
    /// 0-based.
    pub socket: usize,
    /// Bits that changed when the plug went in (`set` = the bit went ON). They also show the resting state while plugged in.
    pub plug: Vec<BitChange>,
    /// Bits that changed when the button was pressed (`set` = the bit went ON when pressed; false means ON at rest, OFF pressed).
    pub button: Vec<BitChange>,
    /// After letting go, the report was the same as before the press.
    pub released_to_rest: bool,
    /// After unplugging, the report was the same as before the plug went in.
    pub unplugged_to_empty: bool,
    /// The button bit differs from every other tested socket's button bit.
    pub unique: bool,
}

/// For each socket, the numbers (1-based) of the other sockets that use any of the same bits (plug or button).
pub fn socket_overlaps(rows: &[SocketResult]) -> Vec<Vec<usize>> {
    let bits = |r: &SocketResult| -> BTreeSet<(usize, u8)> { r.plug.iter().chain(&r.button).map(|b| (b.index, b.bit)).collect() };
    rows.iter()
        .map(|r| {
            let mine = bits(r);
            rows.iter().filter(|o| o.socket != r.socket && !bits(o).is_disjoint(&mine)).map(|o| o.socket + 1).collect()
        })
        .collect()
}

/// Counts what a jog dial sends. Measured on a test machine: each detent is ONE report (data type 0) in which byte 7 is a signed step
/// count (+1 one way, 0xFF = -1 the other, larger magnitudes possible when turned fast) and a per-direction pulse flag is set
/// (byte 3 bit 7 for the positive direction, byte 4 bit 7 for the negative one); about 32 ms later a report with all of that back
/// to rest follows. Reports the guide asked for (Generate Data) or another program asked for carry the resting state and are not
/// counted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JogCounter {
    /// Detents in the positive direction (sum of positive byte 7 values).
    pub positive: u32,
    pub negative: u32,
    /// Reports with a non-zero byte 7.
    pub reports: u32,
    /// The pulse flag of the positive direction (byte 3 bit 7) was seen together with a positive count.
    pub positive_flag: bool,
    pub negative_flag: bool,
    /// Every non-zero value byte 7 took.
    pub values: BTreeSet<u8>,
}

impl JogCounter {
    pub fn feed(&mut self, data: &[u8]) {
        if data.len() < 8 || !is_state_report(data) || data[2] & 0x02 != 0 {
            return;
        }
        let step = data[7] as i8;
        if step == 0 {
            return;
        }
        self.reports += 1;
        self.values.insert(data[7]);
        if step > 0 {
            self.positive += u32::from(step.unsigned_abs());
            self.positive_flag |= data[3] & 0x80 != 0;
        } else {
            self.negative += u32::from(step.unsigned_abs());
            self.negative_flag |= data[4] & 0x80 != 0;
        }
    }
}

/// The verdict on one jog step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JogVerdict {
    /// Turned one way: the sign (+1 or -1) and the detent count.
    Turned { sign: i32, detents: u32 },
    Nothing,
    /// Fewer than [`JOG_MIN_DETENTS`] in the dominant direction.
    TooFew { sign: i32, detents: u32 },
    /// Both directions in roughly equal measure.
    Mixed,
}

pub fn judge_jog(c: &JogCounter) -> JogVerdict {
    let (pos, neg) = (c.positive, c.negative);
    let (dom, minor) = (pos.max(neg), pos.min(neg));
    if dom == 0 {
        return JogVerdict::Nothing;
    }
    if minor * 2 > dom {
        return JogVerdict::Mixed;
    }
    let sign = if pos > neg { 1 } else { -1 };
    if dom < JOG_MIN_DETENTS { JogVerdict::TooFew { sign, detents: dom } } else { JogVerdict::Turned { sign, detents: dom } }
}

/// Shuttle positions seen in byte 8, a signed position (+1..+7 clockwise, -1..-7 counter-clockwise, 0 at rest): the positive
/// ones and the negative ones, as absolute values.
pub fn shuttle_positions(values: &BTreeSet<u8>) -> (BTreeSet<u8>, BTreeSet<u8>) {
    let mut pos = BTreeSet::new();
    let mut neg = BTreeSet::new();
    for &v in values {
        match v as i8 {
            0 => {}
            s if s > 0 => {
                pos.insert(s.unsigned_abs());
            }
            s => {
                neg.insert(s.unsigned_abs());
            }
        }
    }
    (pos, neg)
}

/// The result of reading a shuttle sweep from its sequence of signed byte-8 positions (0 = at rest between).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SweepVerdict {
    /// +1 or -1: the side of the sweep, `None` if no position was reported.
    pub sign: Option<i32>,
    /// The positions (absolute values) reached on that side.
    pub positions: BTreeSet<u8>,
    /// Positions of magnitude at most [`SHUTTLE_OVERSHOOT_MAX`] on the other side right at the end of the sweep (the ring's spring
    /// swinging past zero as it is released), ignored.
    pub overshoot: Vec<i8>,
    /// Real positions on both sides, not explained as an overshoot.
    pub both_sides: bool,
}

/// Largest position on the wrong side that is still treated as the spring's overshoot. The recordings show a single -1 after a
/// clockwise sweep (+...+6, then -1, then rest); a magnitude of 1 is all they justify.
pub const SHUTTLE_OVERSHOOT_MAX: u8 = 1;
/// Most reports of overshoot tolerated.
pub const SHUTTLE_OVERSHOOT_REPORTS: usize = 2;

/// Interpret the signed positions a shuttle step saw, in order (consecutive repeats and rest entries may be included).
pub fn shuttle_sweep(sequence: &[i8]) -> SweepVerdict {
    let moves: Vec<i8> = sequence.iter().copied().filter(|&v| v != 0).collect();
    let pos = moves.iter().filter(|&&v| v > 0).count();
    let neg = moves.len() - pos;
    if moves.is_empty() {
        return SweepVerdict { sign: None, positions: BTreeSet::new(), overshoot: Vec::new(), both_sides: false };
    }
    let sign: i32 = if pos >= neg { 1 } else { -1 };
    let same = |v: i8| (v > 0) == (sign > 0);
    let last_main = moves.iter().rposition(|&v| same(v)).unwrap_or(0);
    let tail: Vec<i8> = moves[last_main + 1..].to_vec();
    let tail_is_overshoot = !tail.is_empty() && tail.len() <= SHUTTLE_OVERSHOOT_REPORTS && tail.iter().all(|&v| !same(v) && v.unsigned_abs() <= SHUTTLE_OVERSHOOT_MAX);
    let (main_part, overshoot): (&[i8], Vec<i8>) = if tail_is_overshoot { (&moves[..=last_main], tail) } else { (&moves[..], Vec::new()) };
    let both_sides = main_part.iter().any(|&v| same(v)) && main_part.iter().any(|&v| !same(v));
    let positions = main_part.iter().filter(|&&v| same(v)).map(|v| v.unsigned_abs()).collect();
    SweepVerdict { sign: Some(sign), positions, overshoot, both_sides }
}

/// Said in the result file: the jog's pulse flags fall inside the key bytes.
pub const JOG_KEY_OVERLAP_NOTE: &str = "The jog dial's pulse flags are byte 3 bit 7 (clockwise, input number 7) and byte 4 bit 7 (counter-clockwise, input number 15). Those bytes also carry keys, so a consumer that maps every bit of bytes 3 to 6 as keys would see jog turns as key presses; the jog's signed step count is byte 7.";

#[derive(Clone, Debug)]
pub struct StepResult {
    pub step: Step,
    pub attempts: Vec<Attempt>,
}

impl StepResult {
    pub fn outcome(&self) -> Outcome {
        self.attempts.last().map_or(Outcome::Skipped, |a| a.outcome)
    }
}

/// What a finished (or stopped) session produced.
#[derive(Debug)]
pub struct SessionOutcome {
    pub results: Vec<StepResult>,
    /// The person never said go, so nothing ran.
    pub not_started: bool,
    /// The person typed quit; the remaining steps are recorded as skipped.
    pub stopped_early: bool,
    /// The end the person said they counted the Switch Interface's sockets from (`left` is assumed until told otherwise).
    pub counting_from: String,
}

// ---------------------------------------------------------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------------------------------------------------------

const TICK_US: i64 = 20_000;
const BASELINE_WAIT_US: i64 = 600_000;
const HOTPLUG_SETTLE_US: i64 = 1_500_000;
const PAIR_MAX_TRIES: u32 = 3;
const OBSERVE_US: i64 = 10_000_000;
const PLUG_SETTLE_US: i64 = 1_000_000;
const HOLD_SETTLE_US: i64 = 600_000;
/// The phase number `begin_phase` gets while the person plugs the push button in before a timing step.
pub const PLUG_PHASE: u32 = 100;

enum Flow {
    Done(Box<Body>),
    Skip,
    Repeat,
    Quit,
}

struct Body {
    outcome: Outcome,
    note: String,
    response: Json,
    bits: Vec<BitChange>,
    reports: Vec<LoggedReport>,
    reports_dropped: usize,
    socket: Option<SocketResult>,
}

struct Engine<'a> {
    hub: &'a mut dyn Hub,
    input: &'a mut dyn Input,
    out: &'a mut dyn Write,
    opts: Options,
    descs: Vec<DeviceDesc>,
    masks: Vec<Vec<u8>>,
    last: Vec<Option<Vec<u8>>>,
    /// (device, byte, bit) -> label of the key it was assigned to earlier in this session.
    assigned: BTreeMap<(usize, usize, u8), String>,
    /// (device, is shuttle, clockwise) -> the sign (+1/-1) the jog or shuttle reported in the step that passed.
    signs: BTreeMap<(usize, bool, bool), i32>,
    /// (device, socket) -> what that socket showed.
    sockets: BTreeMap<(usize, usize), SocketResult>,
    /// Devices whose socket intro was already shown.
    intro_shown: BTreeSet<usize>,
    counting_from: String,
    /// Devices whose lights this tool has switched (and must put back).
    lights_touched: BTreeSet<usize>,
    outputs_touched: BTreeSet<(usize, u8)>,
    /// Times the backlight toggle was sent, per device.
    toggles: BTreeMap<usize, u32>,
    lights_intro_shown: bool,
    step_start_us: i64,
    dropped: usize,
    quit: bool,
}

macro_rules! say {
    ($engine:expr, $($arg:tt)*) => {{
        let _ = writeln!($engine.out, $($arg)*);
        let _ = $engine.out.flush();
    }};
}

fn secs(us: i64) -> i64 {
    (us + 999_999) / 1_000_000
}

fn short_name(desc: &DeviceDesc) -> &'static str {
    match desc.model() {
        Model::JogShuttle => "Jog & Shuttle",
        Model::SwitchInterface12 => "Switch Interface",
        Model::SwitchInterface3 => "XK-3 Switch Interface",
        Model::Unknown => "device",
    }
}

fn fail_body(note: impl Into<String>, reports: Vec<LoggedReport>, dropped: usize) -> Flow {
    Flow::Done(Box::new(Body { outcome: Outcome::Fail, note: note.into(), response: Json::object(), bits: Vec::new(), reports, reports_dropped: dropped, socket: None }))
}

impl<'a> Engine<'a> {
    fn push_log(&mut self, log: &mut Vec<LoggedReport>, item: LoggedReport) {
        if log.len() < MAX_LOGGED_PER_ATTEMPT {
            log.push(item);
        } else {
            self.dropped += 1;
        }
    }

    /// Wait up to `wait_us` for entries, log them, keep each device's last state, and show what changed on watched devices.
    fn pump(&mut self, wait_us: i64, mut log: Option<&mut Vec<LoggedReport>>, watch: &[usize]) -> Vec<(usize, ReportEntry)> {
        let entries = self.hub.poll(wait_us);
        for (d, e) in &entries {
            let t_us = self.hub.to_us(e.qpc);
            if let Some(l) = log.as_deref_mut() {
                self.push_log(l, LoggedReport::from_entry(*d, e, t_us));
            }
            if e.flags & FLAG_ERROR != 0 {
                self.last[*d] = None;
                if watch.contains(d) {
                    say!(self, "  [{}] the {} stopped sending (error {}, Windows error {}).", self.clock(), short_name(&self.descs[*d]), e.error, e.win32_error);
                }
                continue;
            }
            let data = &e.data[..(e.length as usize).min(MAX_REPORT_LEN)];
            if !is_state_report(data) {
                continue;
            }
            if watch.contains(d) {
                if let Some(prev) = &self.last[*d] {
                    let mask = &self.masks[*d];
                    let changes = diff_bits(mask, prev, data);
                    if !changes.is_empty() {
                        let bytes = diff_bytes(mask, prev, data)
                            .iter()
                            .map(|(i, a, b)| format!("byte {i}: {a:02X} -> {b:02X}"))
                            .collect::<Vec<_>>()
                            .join("; ");
                        let line = format!("  [{}] {}: {} ({bytes})", self.clock_at(t_us), short_name(&self.descs[*d]), describe_bits(&changes));
                        say!(self, "{line}");
                    }
                }
            }
            self.last[*d] = Some(data.to_vec());
        }
        entries
    }

    fn clock(&self) -> String {
        self.clock_at(self.hub.now_us())
    }

    /// Seconds since the step began, for the live lines.
    fn clock_at(&self, us: i64) -> String {
        format!("{:>5.2} s", (us - self.step_start_us) as f64 / 1e6)
    }

    /// Check for a typed command, then pump once.
    fn tick(&mut self, step: &Step, attempt: u32, log: &mut Vec<LoggedReport>, watch: &[usize]) -> Result<Vec<(usize, ReportEntry)>, Flow> {
        match self.input.poll(&step.id, attempt) {
            Some(Command::Skip) => return Err(Flow::Skip),
            Some(Command::Repeat) => return Err(Flow::Repeat),
            Some(Command::Quit) => return Err(Flow::Quit),
            Some(c @ (Command::Left | Command::Right)) => self.note_origin(c),
            Some(Command::Go) | None => {}
        }
        Ok(self.pump(TICK_US, Some(log), watch))
    }

    fn note_origin(&mut self, c: Command) {
        self.counting_from = if c == Command::Right { "right" } else { "left" }.to_string();
        say!(self, "  Noted: you count the Switch Interface's sockets from the {}.", self.counting_from);
    }

    #[allow(clippy::too_many_arguments)]
    /// Wait until the device's report has changed from `reference` and then stayed put for `settle_us`; returns that final state.
    /// A change that goes back to `reference` does not count. Times out with a plain message saying what it waited for.
    fn wait_for_change(
        &mut self,
        step: &Step,
        attempt: u32,
        dev: usize,
        log: &mut Vec<LoggedReport>,
        reference: &[u8],
        settle_us: i64,
        what: &str,
    ) -> Result<Vec<u8>, Flow> {
        let mask = self.masks[dev].clone();
        let start = self.hub.now_us();
        let deadline = start + self.opts.timeout_us;
        let mut next_remind = start + self.opts.reminder_us;
        let mut latest = reference.to_vec();
        let mut last_change = start;
        loop {
            let entries = self.tick(step, attempt, log, &[dev])?;
            for (d, e) in entries {
                if d != dev {
                    continue;
                }
                if e.flags & FLAG_ERROR != 0 {
                    return Err(fail_body(
                        format!("the device stopped sending while waiting for {what} (error {}, Windows error {}); it may have been unplugged", e.error, e.win32_error),
                        std::mem::take(log),
                        std::mem::take(&mut self.dropped),
                    ));
                }
                let data = &e.data[..(e.length as usize).min(MAX_REPORT_LEN)];
                if is_state_report(data) && !diff_bits(&mask, &latest, data).is_empty() {
                    latest = data.to_vec();
                    last_change = self.hub.to_us(e.qpc);
                }
            }
            let now = self.hub.now_us();
            if !diff_bits(&mask, reference, &latest).is_empty() && now - last_change >= settle_us {
                return Ok(latest);
            }
            if now >= deadline {
                return Err(fail_body(
                    format!("nothing changed within {} s while waiting for {what}", secs(self.opts.timeout_us)),
                    std::mem::take(log),
                    std::mem::take(&mut self.dropped),
                ));
            }
            self.remind(&mut next_remind, deadline, what);
        }
    }

    /// Make sure the device is open and get its current state with a Generate Data request; that state is the step's baseline.
    fn prepare(&mut self, dev: usize, log: &mut Vec<LoggedReport>) -> Result<Vec<u8>, String> {
        if !self.hub.is_open(dev) {
            say!(self, "  (the {} is not open; trying to open it again)", short_name(&self.descs[dev]));
            self.hub.reopen(dev).map_err(|e| format!("the {} is not connected ({e})", short_name(&self.descs[dev])))?;
        }
        self.last[dev] = None;
        self.hub
            .request_report(dev)
            .map_err(|e| format!("could not ask the {} for its current state ({e})", short_name(&self.descs[dev])))?;
        let end = self.hub.now_us() + BASELINE_WAIT_US;
        while self.last[dev].is_none() && self.hub.now_us() < end {
            self.pump(TICK_US, Some(&mut *log), &[]);
        }
        self.last[dev].clone().ok_or_else(|| format!("the {} did not answer the request for its current state", short_name(&self.descs[dev])))
    }

    fn announce(&mut self, step: &Step, total: usize, what_next: &str) {
        say!(self, "");
        say!(self, "============================================================");
        say!(self, "Step {} of {total}: {}", step.number, step.title);
        say!(self, "  >> {}", step.prompt);
        say!(
            self,
            "  {what_next} Waiting up to {} s. Type  skip  and press Enter to skip this step, or  repeat  to start it over.",
            secs(self.opts.timeout_us)
        );
    }

    fn remind(&mut self, next: &mut i64, deadline: i64, what: &str) {
        let now = self.hub.now_us();
        if now >= *next {
            say!(self, "  ...still waiting for {what} ({} s left). Type  skip  and press Enter to skip.", secs(deadline - now).max(0));
            *next = now + self.opts.reminder_us;
        }
    }

    // ---- press and release one key or input, jog, shuttle, observe ------------------------------------------------------------

    fn attempt_press(&mut self, step: &Step, attempt: u32) -> Flow {
        let dev = step.device;
        let mut log = Vec::new();
        self.hub.begin_phase(step, 0);
        let baseline = match self.prepare(dev, &mut log) {
            Ok(b) => b,
            Err(note) => return fail_body(note, log, std::mem::take(&mut self.dropped)),
        };
        let mut tracker = Tracker::new(baseline, self.masks[dev].clone());
        let model = self.descs[dev].model();
        let settle = matches!(step.kind, StepKind::Jog { .. });
        let observe = matches!(step.kind, StepKind::Observe);
        let start = self.hub.now_us();
        self.step_start_us = start;
        let timeout = if observe { self.opts.timeout_us.min(OBSERVE_US) } else { self.opts.timeout_us };
        let deadline = start + timeout;
        let mut next_remind = start + self.opts.reminder_us;
        let mut last_active_us = start;
        let mut told_release = false;
        let mut jog = JogCounter::default();
        let mut shuttle_seq: Vec<i8> = Vec::new();
        let what = match &step.kind {
            StepKind::Press { label, .. } => format!("{label} to be pressed"),
            StepKind::Jog { .. } => "the jog dial to turn".to_string(),
            StepKind::Shuttle { .. } => "the shuttle ring to move".to_string(),
            _ => "any key or control to be used".to_string(),
        };
        loop {
            let entries = match self.tick(step, attempt, &mut log, &[dev]) {
                Ok(e) => e,
                Err(flow) => return flow,
            };
            for (d, e) in entries {
                if d != dev {
                    continue;
                }
                if e.flags & FLAG_ERROR != 0 {
                    return fail_body(
                        format!("the device stopped sending while waiting (error {}, Windows error {}); it may have been unplugged", e.error, e.win32_error),
                        log,
                        std::mem::take(&mut self.dropped),
                    );
                }
                let data = &e.data[..(e.length as usize).min(MAX_REPORT_LEN)];
                if is_state_report(data) {
                    // The quiet time counts from the last report that differs from the starting state. Reports that merely repeat
                    // the resting state (the guide's own Generate Data answer, or another program's requests) must not restart it.
                    if !tracker.feed(data).is_empty() {
                        last_active_us = self.hub.to_us(e.qpc);
                    }
                    jog.feed(data);
                    if data.len() > 8 && data[8] as i8 != shuttle_seq.last().copied().unwrap_or(0) {
                        shuttle_seq.push(data[8] as i8);
                    }
                    if tracker.seen_active && !told_release && !settle && !observe {
                        told_release = true;
                        say!(self, "  Got it. Now let go (or open it again) and wait a moment.");
                    }
                }
            }
            let now = self.hub.now_us();
            let done = if observe {
                now >= deadline
            } else if settle {
                tracker.seen_active && now - last_active_us >= self.opts.quiet_us
            } else {
                tracker.seen_active && !tracker.is_active()
            };
            if done {
                break;
            }
            if now >= deadline {
                let note = if tracker.seen_active && settle {
                    format!(
                        "the dial was still moving when the {} s ran out; turn a few clicks, then keep it still for {} s",
                        secs(timeout),
                        secs(self.opts.quiet_us)
                    )
                } else if tracker.seen_active && !observe {
                    format!("it was pressed but not let go within {} s; the bits that changed were {}", secs(timeout), describe_bits(&tracker.bits()))
                } else {
                    format!("nothing arrived from the {} within {} s (nobody used it, or the device is not sending)", short_name(&self.descs[dev]), secs(timeout))
                };
                return fail_body(note, log, std::mem::take(&mut self.dropped));
            }
            self.remind(&mut next_remind, deadline, &what);
        }
        let bits = tracker.bits();
        let (outcome, note, mut response) = self.judge_press(step, model, &bits, &jog, &shuttle_seq);
        if let Json::Obj(_) = response {
            response = response.with("changed_bits", bits_json(&bits));
        }
        Flow::Done(Box::new(Body { outcome, note, response, bits, reports: log, reports_dropped: std::mem::take(&mut self.dropped), socket: None }))
    }

    fn judge_press(&mut self, step: &Step, model: Model, bits: &[BitChange], jog: &JogCounter, shuttle: &[i8]) -> (Outcome, String, Json) {
        let dev = step.device;
        match &step.kind {
            StepKind::Press { label, .. } => {
                // Forget this key's earlier assignment (a repeat), then see whether the bit is already another key's.
                self.assigned.retain(|k, v| !(k.0 == dev && v == label));
                if bits.len() != 1 {
                    return (
                        Outcome::Fail,
                        format!("{} bits changed ({}); expected exactly one. Repeat it, touching only that one control.", bits.len(), describe_bits(bits)),
                        Json::object(),
                    );
                }
                let b = bits[0];
                if let Some(other) = self.assigned.get(&(dev, b.index, b.bit)) {
                    return (
                        Outcome::Fail,
                        format!("this changed byte {} bit {}, which is also what {other} did; probably the previous control again", b.index, b.bit),
                        Json::object(),
                    );
                }
                self.assigned.insert((dev, b.index, b.bit), label.clone());
                let n = input_number(b.index, b.bit);
                let documented = n.is_some_and(|n| model.documented_inputs().contains(&n));
                let note = format!(
                    "{label} = byte {} bit {} (input number {}){}{}",
                    b.index,
                    b.bit,
                    n.map_or("?".to_string(), |n| n.to_string()),
                    if b.set { "" } else { ", which goes OFF when pressed" },
                    if documented { ", as documented" } else { "; NOT where the documentation puts it" }
                );
                (Outcome::Pass, note, Json::object().with("input", n).with("in_documented_set", documented))
            }
            StepKind::Jog { clockwise } => self.judge_jog_step(dev, *clockwise, jog),
            StepKind::Shuttle { clockwise } => self.judge_shuttle_step(dev, *clockwise, shuttle),
            _ => {
                if bits.is_empty() {
                    (Outcome::Fail, "nothing was used".to_string(), Json::object())
                } else {
                    (Outcome::Pass, format!("recorded changes: {}", describe_bits(bits)), Json::object())
                }
            }
        }
    }

    /// Jog: count detents and net direction; clockwise is expected to be positive. Reports a sign that disagrees with that, and a
    /// second step that reports the same sign as the first, instead of accepting them silently.
    fn judge_jog_step(&mut self, dev: usize, clockwise: bool, jog: &JogCounter) -> (Outcome, String, Json) {
        self.signs.remove(&(dev, false, clockwise));
        let word = if clockwise { "clockwise" } else { "counter-clockwise" };
        let response = Json::object()
            .with("detents_positive", jog.positive)
            .with("detents_negative", jog.negative)
            .with("reports_with_a_step", jog.reports)
            .with("byte_7_values", jog.values.iter().copied().collect::<Vec<u8>>())
            .with("positive_flag_byte_3_bit_7_seen", jog.positive_flag)
            .with("negative_flag_byte_4_bit_7_seen", jog.negative_flag)
            .with("key_byte_overlap", JOG_KEY_OVERLAP_NOTE);
        let counts = format!("{} detent(s) positive, {} negative", jog.positive, jog.negative);
        match judge_jog(jog) {
            JogVerdict::Nothing => (Outcome::Fail, "no jog detent was counted (byte 7 never became non-zero)".to_string(), response),
            JogVerdict::Mixed => (
                Outcome::Fail,
                format!("the dial was turned both ways in similar amounts ({counts}); turn it only {word} this time"),
                response,
            ),
            JogVerdict::TooFew { detents, .. } => (
                Outcome::Fail,
                format!("only {detents} detent(s) in one direction ({counts}); turn at least {JOG_MIN_DETENTS} clicks"),
                response,
            ),
            JogVerdict::Turned { sign, detents } => {
                let other = self.signs.get(&(dev, false, !clockwise)).copied();
                let flag = if sign > 0 { "byte 3 bit 7" } else { "byte 4 bit 7" };
                let flag_seen = if sign > 0 { jog.positive_flag } else { jog.negative_flag };
                let response = response.with("direction", if sign > 0 { "positive" } else { "negative" }).with("expected_sign", if clockwise { 1 } else { -1 });
                if other == Some(sign) {
                    return (
                        Outcome::Fail,
                        format!(
                            "this reported the same direction ({}) as the {} step; one of the two was turned the wrong way. Repeat this step turning the opposite way",
                            if sign > 0 { "positive" } else { "negative" },
                            if clockwise { "counter-clockwise" } else { "clockwise" }
                        ),
                        response,
                    );
                }
                self.signs.insert((dev, false, clockwise), sign);
                let mut note = format!(
                    "jog {word}: {detents} detent(s) of step {}{} ({counts}); direction flag {flag} {}",
                    if sign > 0 { "+1" } else { "-1" },
                    if jog.values.iter().any(|&v| (v as i8).unsigned_abs() > 1) { " or more" } else { "" },
                    if flag_seen { "seen" } else { "NOT seen" }
                );
                if (sign > 0) != clockwise {
                    note.push_str(&format!(
                        "; NOTE: clockwise is expected to be positive, but {word} reported {}. Recorded as seen",
                        if sign > 0 { "positive" } else { "negative" }
                    ));
                }
                let matches = (sign > 0) == clockwise;
                (Outcome::Pass, note, response.with("matches_expectation", matches))
            }
        }
    }

    /// Shuttle: byte 8 is the signed position (+1..+7 / -1..-7, 0 at rest). Lists the positions that were actually reached. A single
    /// -1 (or +1) right at the end of a sweep is the spring overshooting zero as the ring is released; it is ignored and noted.
    fn judge_shuttle_step(&mut self, dev: usize, clockwise: bool, sequence: &[i8]) -> (Outcome, String, Json) {
        self.signs.remove(&(dev, true, clockwise));
        let word = if clockwise { "clockwise" } else { "counter-clockwise" };
        let sweep = shuttle_sweep(sequence);
        let as_i64 = |v: &i8| Json::Int(i64::from(*v));
        let mut response = Json::object()
            .with("byte_8_sequence", Json::Arr(sequence.iter().map(as_i64).collect()))
            .with("overshoot_ignored", Json::Arr(sweep.overshoot.iter().map(as_i64).collect()))
            .with("expected_sign", if clockwise { 1 } else { -1 });
        let Some(sign) = sweep.sign else {
            return (Outcome::Fail, "the shuttle ring never reported a position (byte 8 stayed 0)".to_string(), response);
        };
        let signed = |v: &u8| Json::Int(i64::from(*v) * i64::from(sign));
        response = response.with("positions_reached", Json::Arr(sweep.positions.iter().map(signed).collect()));
        if sweep.both_sides {
            let mut seen: Vec<i8> = sequence.iter().copied().filter(|&v| v != 0).collect();
            seen.sort_unstable();
            seen.dedup();
            let list = seen.iter().map(|v| format!("{v:+}")).collect::<Vec<_>>().join(", ");
            return (Outcome::Fail, format!("the ring reported positions on both sides ({list}); move it only {word} this time"), response);
        }
        if self.signs.get(&(dev, true, !clockwise)) == Some(&sign) {
            return (
                Outcome::Fail,
                format!(
                    "this reported the same side ({}) as the {} step; one of the two was moved the wrong way. Repeat this step moving the ring the opposite way",
                    if sign > 0 { "positive" } else { "negative" },
                    if clockwise { "counter-clockwise" } else { "clockwise" }
                ),
                response,
            );
        }
        self.signs.insert((dev, true, clockwise), sign);
        let reached = sweep.positions.iter().map(|v| format!("{}{v}", if sign > 0 { "+" } else { "-" })).collect::<Vec<_>>().join(", ");
        let mut note = format!(
            "shuttle {word}: reached {} position(s) ({reached}) and returned to rest. The device reports positions 1 to 7; positions not listed were not reached in this step",
            sweep.positions.len()
        );
        if !sweep.overshoot.is_empty() {
            let o = sweep.overshoot.iter().map(|v| format!("{v:+}")).collect::<Vec<_>>().join(", ");
            note.push_str(&format!("; a brief overshoot to the other side ({o}) was seen as the ring sprang back to rest, and ignored"));
        }
        if (sign > 0) != clockwise {
            note.push_str(&format!("; NOTE: clockwise is expected to be positive, but {word} reported {}. Recorded as seen", if sign > 0 { "positive" } else { "negative" }));
        }
        let matches = (sign > 0) == clockwise;
        (Outcome::Pass, note, response.with("matches_expectation", matches))
    }

    // ---- two controls at once -------------------------------------------------------------------------------------------------

    fn attempt_pair(&mut self, step: &Step, attempt: u32, reps: u32) -> Flow {
        let devs: Vec<usize> = std::iter::once(step.device).chain(step.partner).collect();
        let cross = devs.len() == 2;
        let poll = self.descs[step.device].model().polling_interval_us();
        let mut log = Vec::new();
        let mut rounds: Vec<RoundStats> = Vec::new();
        let mut rounds_json = Vec::new();
        // Per device: the last report caused by the person (time, device clock) and the smallest gaps between consecutive ones.
        let mut last_seen: BTreeMap<usize, (i64, Option<u32>)> = BTreeMap::new();
        let mut min_gap: BTreeMap<usize, i64> = BTreeMap::new();
        let mut min_device_gap: BTreeMap<usize, i64> = BTreeMap::new();
        if let StepKind::Pair { plug_socket: Some(sock), .. } = step.kind {
            for &d in &devs {
                if self.descs[d].model() != Model::SwitchInterface12 {
                    continue;
                }
                let empty = match self.prepare(d, &mut log) {
                    Ok(b) => b,
                    Err(note) => return fail_body(note, log, std::mem::take(&mut self.dropped)),
                };
                self.step_start_us = self.hub.now_us();
                self.hub.begin_phase(step, PLUG_PHASE);
                say!(self, "");
                say!(self, "  >> First plug the push button into socket {} of the Switch Interface, and leave it plugged in.", sock + 1);
                if let Err(flow) = self.wait_for_change(step, attempt, d, &mut log, &empty, PLUG_SETTLE_US, "the button to be plugged in") {
                    return flow;
                }
            }
        }
        for round in 0..reps {
            let mut tries = 0;
            loop {
                tries += 1;
                self.hub.begin_phase(step, round);
                let mut trackers = Vec::new();
                for &d in &devs {
                    match self.prepare(d, &mut log) {
                        Ok(b) => trackers.push(Tracker::new(b, self.masks[d].clone())),
                        Err(note) => return fail_body(note, log, std::mem::take(&mut self.dropped)),
                    }
                }
                let start = self.hub.now_us();
                self.step_start_us = start;
                say!(self, "");
                let round_prompt = if matches!(step.kind, StepKind::Pair { plug_socket: Some(_), .. }) {
                    "Press the button and any key on the Jog & Shuttle at the very same moment, then let go of both."
                } else {
                    step.prompt.as_str()
                };
                say!(self, "  Round {} of {reps}: {round_prompt}", round + 1);
                let deadline = start + self.opts.timeout_us;
                let mut next_remind = start + self.opts.reminder_us;
                let mut firsts: Vec<FirstSeen> = Vec::new();
                loop {
                    let entries = match self.tick(step, attempt, &mut log, &devs) {
                        Ok(e) => e,
                        Err(flow) => return flow,
                    };
                    for (d, e) in entries {
                        let Some(k) = devs.iter().position(|&x| x == d) else { continue };
                        if e.flags & FLAG_ERROR != 0 {
                            return fail_body(format!("the {} stopped sending (error {}); it may have been unplugged", short_name(&self.descs[d]), e.error), log, std::mem::take(&mut self.dropped));
                        }
                        let data = &e.data[..(e.length as usize).min(MAX_REPORT_LEN)];
                        if !is_state_report(data) {
                            continue;
                        }
                        if data[2] & 0x02 == 0 {
                            let t = self.hub.to_us(e.qpc);
                            let ms = (e.flags & FLAG_HAS_DEVICE_MS != 0).then_some(e.device_ms);
                            if let Some((previous_t, previous_ms)) = last_seen.insert(d, (t, ms)) {
                                if t > previous_t {
                                    min_gap.entry(d).and_modify(|m| *m = (*m).min(t - previous_t)).or_insert(t - previous_t);
                                }
                                if let (Some(a), Some(b)) = (previous_ms, ms) {
                                    let g = i64::from(b.wrapping_sub(a) as i32).abs();
                                    if g > 0 {
                                        min_device_gap.entry(d).and_modify(|m| *m = (*m).min(g)).or_insert(g);
                                    }
                                }
                            }
                        }
                        for c in trackers[k].feed(data) {
                            if !firsts.iter().any(|f| f.device == d && f.key == (c.index, c.bit)) {
                                firsts.push(FirstSeen {
                                    device: d,
                                    key: (c.index, c.bit),
                                    t_us: self.hub.to_us(e.qpc),
                                    device_ms: (e.flags & FLAG_HAS_DEVICE_MS != 0).then_some(e.device_ms),
                                    sequence: e.sequence,
                                });
                            }
                        }
                    }
                    let any_seen = trackers.iter().any(|t| t.seen_active);
                    let all_rest = trackers.iter().all(|t| !t.is_active());
                    if any_seen && all_rest {
                        break;
                    }
                    let now = self.hub.now_us();
                    if now >= deadline {
                        let note = if any_seen {
                            "a control was pressed but not let go in time".to_string()
                        } else {
                            format!("nothing arrived within {} s (round {} of {reps}); nobody pressed anything", secs(self.opts.timeout_us), round + 1)
                        };
                        return fail_body(note, log, std::mem::take(&mut self.dropped));
                    }
                    self.remind(&mut next_remind, deadline, "both controls to be pressed together");
                }
                if let Some(stats) = analyze_round(&firsts, cross, poll) {
                    let describe = |f: &FirstSeen| format!("byte {} bit {}", f.key.0, f.key.1);
                    say!(
                        self,
                        "  Recorded: {} and {} arrived {} (gap {:.2} ms).",
                        describe(&stats.first),
                        describe(&stats.second),
                        if stats.same_report { "in the same report" } else { "in separate reports" },
                        stats.gap_us as f64 / 1000.0
                    );
                    rounds_json.push(
                        Json::object()
                            .with("round", round + 1)
                            .with("tries", tries)
                            .with("first_device", stats.first.device)
                            .with("first_control", describe(&stats.first))
                            .with("second_device", stats.second.device)
                            .with("second_control", describe(&stats.second))
                            .with("same_report", stats.same_report)
                            .with("gap_us", stats.gap_us)
                            .with("device_gap_ms", stats.device_gap_ms)
                            .with("within_polling_interval", stats.within_polling_interval),
                    );
                    rounds.push(stats);
                    break;
                }
                if tries >= PAIR_MAX_TRIES {
                    return fail_body(
                        format!("round {} of {reps}: only one control was seen {PAIR_MAX_TRIES} times; both must be pressed together", round + 1),
                        log,
                        std::mem::take(&mut self.dropped),
                    );
                }
                say!(self, "  I saw only one control. Press both together this time, and let go of both.");
            }
        }
        let same = rounds.iter().filter(|r| r.same_report).count();
        let gaps: Vec<i64> = rounds.iter().map(|r| r.gap_us).collect();
        let within = rounds.iter().filter(|r| r.within_polling_interval == Some(true)).count();
        let response = Json::object()
            .with("rounds", rounds_json)
            .with("same_report", same)
            .with("separate_reports", rounds.len() - same)
            .with("min_gap_us", gaps.iter().copied().min())
            .with("max_gap_us", gaps.iter().copied().max())
            .with("polling_interval_us", if cross { None } else { poll })
            .with("within_polling_interval", if cross || poll.is_none() { None } else { Some(within) })
            .with(
                "smallest_gap_between_reports",
                Json::Arr(
                    min_gap
                        .iter()
                        .map(|(d, g)| Json::object().with("device", *d).with("gap_us", *g).with("device_gap_ms", min_device_gap.get(d).copied()))
                        .collect(),
                ),
            );
        let smallest = min_gap
            .iter()
            .map(|(d, g)| {
                format!(
                    "; smallest gap between consecutive reports from the {}: {:.1} ms{}",
                    short_name(&self.descs[*d]),
                    *g as f64 / 1000.0,
                    min_device_gap.get(d).map_or(String::new(), |m| format!(" (device clock {m} ms)"))
                )
            })
            .collect::<String>();
        let note = format!(
            "{} rounds: {same} in one report, {} in separate reports{}",
            rounds.len(),
            rounds.len() - same,
            match (gaps.iter().copied().min(), gaps.iter().copied().max()) {
                (Some(a), Some(b)) => format!(" (gaps {:.2} to {:.2} ms)", a as f64 / 1000.0, b as f64 / 1000.0),
                _ => String::new(),
            }
        );
        let note = format!("{note}{smallest}");
        Flow::Done(Box::new(Body { outcome: Outcome::Pass, note, response, bits: Vec::new(), reports: log, reports_dropped: std::mem::take(&mut self.dropped), socket: None }))
    }

    // ---- hot-plug -------------------------------------------------------------------------------------------------------------

    fn attempt_hotplug(&mut self, step: &Step, attempt: u32) -> Flow {
        let dev = step.device;
        let name = short_name(&self.descs[dev]);
        let mut log = Vec::new();
        self.hub.begin_phase(step, 0);
        if let Err(note) = self.prepare(dev, &mut log) {
            return fail_body(note, log, std::mem::take(&mut self.dropped));
        }
        let gen_before = self.hub.watch_generation();
        let start = self.hub.now_us();
        self.step_start_us = start;
        let timeout = self.opts.timeout_us.max(60_000_000.min(self.opts.timeout_us * 2));
        say!(self, "");
        say!(self, "  Stage 1 of 2: UNPLUG the {name} now (pull its USB cable out). I am waiting up to {} s.", secs(timeout));
        let deadline = start + timeout;
        let mut next_remind = start + self.opts.reminder_us;
        let mut errors = 0u32;
        let mut error_codes: Vec<(i32, u32)> = Vec::new();
        let mut settle_until: Option<i64> = None;
        loop {
            let entries = match self.tick(step, attempt, &mut log, &[]) {
                Ok(e) => e,
                Err(flow) => return flow,
            };
            for (d, e) in entries {
                if d == dev && e.flags & FLAG_ERROR != 0 {
                    errors += 1;
                    error_codes.push((e.error, e.win32_error));
                    settle_until.get_or_insert(self.hub.now_us() + HOTPLUG_SETTLE_US);
                    say!(self, "  [{}] the {name} stopped sending (error {}, Windows error {}).", self.clock(), e.error, e.win32_error);
                }
            }
            let now = self.hub.now_us();
            if let Some(t) = settle_until {
                if now >= t {
                    break;
                }
            } else if now >= deadline {
                return fail_body(
                    format!("the {name} was not unplugged within {} s: no removal was seen (watch counter {} -> {})", secs(timeout), gen_before, self.hub.watch_generation()),
                    log,
                    std::mem::take(&mut self.dropped),
                );
            } else {
                self.remind(&mut next_remind, deadline, &format!("the {name} to be unplugged"));
            }
        }
        let gen_removed = self.hub.watch_generation();
        let removal_fired = gen_removed > gen_before;
        say!(self, "  Removal seen: {errors} error entr{}, watch event {}.", if errors == 1 { "y" } else { "ies" }, if removal_fired { "fired" } else { "did NOT fire" });
        self.hub.begin_phase(step, 1);
        self.step_start_us = self.hub.now_us();
        say!(self, "  Stage 2 of 2: now PLUG the {name} back in. I am waiting up to {} s.", secs(timeout));
        let deadline = self.hub.now_us() + timeout;
        let mut next_remind = self.hub.now_us() + self.opts.reminder_us;
        let mut reopened = false;
        let mut last_error = String::new();
        let mut next_try = 0i64;
        loop {
            match self.input.poll(&step.id, attempt) {
                Some(Command::Skip) => return Flow::Skip,
                Some(Command::Repeat) => return Flow::Repeat,
                Some(Command::Quit) => return Flow::Quit,
                _ => {}
            }
            self.pump(TICK_US, Some(&mut log), &[]);
            let now = self.hub.now_us();
            if now >= next_try {
                next_try = now + 250_000;
                match self.hub.reopen(dev) {
                    Ok(()) => {
                        reopened = true;
                        break;
                    }
                    Err(e) => last_error = e,
                }
            }
            if now >= deadline {
                break;
            }
            self.remind(&mut next_remind, deadline, &format!("the {name} to be plugged back in"));
        }
        let mut answered = false;
        if reopened {
            self.last[dev] = None;
            say!(self, "  The {name} is back; asking it for its state.");
            let end = self.hub.now_us() + HOTPLUG_SETTLE_US;
            let mut asked = self.hub.request_report(dev).is_ok();
            while self.hub.now_us() < end {
                if !asked {
                    asked = self.hub.request_report(dev).is_ok();
                }
                self.pump(TICK_US, Some(&mut log), &[]);
                answered |= self.last[dev].is_some();
            }
        }
        let arrival_fired = self.hub.watch_generation() > gen_removed;
        let mut problems = Vec::new();
        if errors != 1 {
            problems.push(format!("expected exactly one error entry on removal, saw {errors}"));
        }
        if !removal_fired {
            problems.push("the watch did not fire on removal".to_string());
        }
        if !arrival_fired {
            problems.push("the watch did not fire on arrival".to_string());
        }
        if !reopened {
            problems.push(format!("the device could not be opened again within {} s ({last_error})", secs(timeout)));
        } else if !answered {
            problems.push("the reopened device did not answer Generate Data".to_string());
        }
        let outcome = if problems.is_empty() { Outcome::Pass } else { Outcome::Fail };
        let note = if problems.is_empty() {
            "one error entry on removal, the watch fired on removal and on arrival, and the device reopened and answered".to_string()
        } else {
            problems.join("; ")
        };
        let response = Json::object()
            .with("error_entries", errors)
            .with("error_codes", Json::Arr(error_codes.iter().map(|(e, w)| Json::object().with("error", *e).with("win32_error", *w)).collect()))
            .with("watch_fired_on_removal", removal_fired)
            .with("watch_fired_on_arrival", arrival_fired)
            .with("reopened", reopened)
            .with("answered_generate_data", answered);
        Flow::Done(Box::new(Body { outcome, note, response, bits: Vec::new(), reports: log, reports_dropped: std::mem::take(&mut self.dropped), socket: None }))
    }

    // ---- one Switch Interface socket, tested with one push button --------------------------------------------------------------

    /// Four phases for one socket: plug the button in, press and hold it, let go, unplug it. After each the report is allowed to
    /// settle; the differences between the settled states give the plug bits, the button bit and its polarity.
    fn attempt_socket(&mut self, step: &Step, attempt: u32, socket: usize) -> Flow {
        let dev = step.device;
        let n = socket + 1;
        let mut log = Vec::new();
        let mask = self.masks[dev].clone();
        let empty = match self.prepare(dev, &mut log) {
            Ok(b) => b,
            Err(note) => return fail_body(note, log, std::mem::take(&mut self.dropped)),
        };
        self.step_start_us = self.hub.now_us();
        self.hub.begin_phase(step, 0);
        say!(self, "  >> Plug the push button into socket {n} now.");
        let plugged = match self.wait_for_change(step, attempt, dev, &mut log, &empty, PLUG_SETTLE_US, "the button to be plugged in") {
            Ok(s) => s,
            Err(flow) => return flow,
        };
        let plug = diff_bits(&mask, &empty, &plugged);
        say!(self, "  Plugging in changed: {}. Leave it plugged in.", describe_bits(&plug));

        self.hub.begin_phase(step, 1);
        say!(self, "  >> Now press the button and HOLD it down.");
        let held = match self.wait_for_change(step, attempt, dev, &mut log, &plugged, HOLD_SETTLE_US, "the button to be pressed and held") {
            Ok(s) => s,
            Err(flow) => return flow,
        };
        let button = diff_bits(&mask, &plugged, &held);

        self.hub.begin_phase(step, 2);
        say!(self, "  >> Now LET GO of the button (leave it plugged in).");
        let released = match self.wait_for_change(step, attempt, dev, &mut log, &held, HOLD_SETTLE_US, "the button to be let go") {
            Ok(s) => s,
            Err(flow) => return flow,
        };
        let released_to_rest = diff_bits(&mask, &plugged, &released).is_empty();

        self.hub.begin_phase(step, 3);
        say!(self, "  >> Now UNPLUG the button from socket {n}.");
        let unplugged = match self.wait_for_change(step, attempt, dev, &mut log, &released, PLUG_SETTLE_US, "the button to be unplugged") {
            Ok(s) => s,
            Err(flow) => return flow,
        };
        let unplugged_to_empty = diff_bits(&mask, &empty, &unplugged).is_empty();

        self.sockets.remove(&(dev, socket));
        let clash = button.first().and_then(|b| {
            self.sockets
                .iter()
                .find(|((d, s), r)| *d == dev && *s != socket && r.button.iter().any(|x| (x.index, x.bit) == (b.index, b.bit)))
                .map(|((_, s), _)| s + 1)
        });
        let result = SocketResult { socket, plug: plug.clone(), button: button.clone(), released_to_rest, unplugged_to_empty, unique: button.len() == 1 && clash.is_none() };
        self.sockets.insert((dev, socket), result.clone());
        let response = Json::object()
            .with("counting_from", self.counting_from.as_str())
            .with("plug_changes", bits_json(&plug))
            .with("button_changes", bits_json(&button))
            .with("released_to_rest", released_to_rest)
            .with("unplugged_to_empty", unplugged_to_empty);
        let mut notes = Vec::new();
        if !released_to_rest {
            notes.push("after letting go the report did not return to what it was before the press".to_string());
        }
        if !unplugged_to_empty {
            notes.push("after unplugging the report did not return to the empty state".to_string());
        }
        let (outcome, mut note) = if button.len() != 1 {
            (Outcome::Fail, format!("{} bits changed when the button was pressed ({}); expected exactly one", button.len(), describe_bits(&button)))
        } else if let Some(other) = clash {
            (Outcome::Fail, format!("the button bit (byte {} bit {}) is the same one socket {other}'s button used; it is not unique to this socket", button[0].index, button[0].bit))
        } else {
            let b = button[0];
            (
                Outcome::Pass,
                format!(
                    "socket {n}: plugging in changed {}; the button is byte {} bit {} (input number {}), which goes {} when pressed (rests {} while plugged in)",
                    if plug.is_empty() { "nothing".to_string() } else { describe_bits(&plug) },
                    b.index,
                    b.bit,
                    input_number(b.index, b.bit).map_or("?".to_string(), |v| v.to_string()),
                    if b.set { "ON" } else { "OFF" },
                    if b.set { "OFF" } else { "ON" }
                ),
            )
        };
        if !notes.is_empty() {
            note.push_str(&format!("; NOTE: {}", notes.join("; ")));
        }
        Flow::Done(Box::new(Body { outcome, note, response, bits: button, reports: log, reports_dropped: std::mem::take(&mut self.dropped), socket: Some(result) }))
    }

    // ---- lights ---------------------------------------------------------------------------------------------------------------

    /// Send one light report: refused unless [`lights::validate_light_report`] accepts it for the device's model, so nothing that
    /// writes EEPROM or changes identity or mode can reach a device. Returns whether it succeeded and the record for the result file.
    fn send_light(&mut self, dev: usize, cmd: &LightCommand) -> (bool, Json) {
        let model = self.descs[dev].model();
        let result = lights::validate_light_report(light_model(model), &cmd.bytes).and_then(|()| self.hub.send_light(dev, &cmd.bytes));
        if result.is_ok() && cmd.bytes.get(1) == Some(&lights::CMD_TOGGLE_BACKLIGHTS) {
            *self.toggles.entry(dev).or_insert(0) += 1;
        }
        if let (Some(&lights::CMD_SET_LED), Some(&out @ (lights::LED_OUT1 | lights::LED_OUT2))) = (cmd.bytes.get(1), cmd.bytes.get(2)) {
            self.outputs_touched.insert((dev, out));
        }
        self.lights_touched.insert(dev);
        let record = Json::object().with("label", cmd.label.as_str()).with("bytes", hex(&cmd.bytes)).with(
            "write",
            match &result {
                Ok(()) => "ok".to_string(),
                Err(e) => format!("error: {e}"),
            },
        );
        (result.is_ok(), record)
    }

    /// Wait for the answer to a light question: from the simulation if there is one, else typed by the person.
    /// `Ok(None)` is a timeout.
    fn wait_for_answer(&mut self, step: &Step, attempt: u32, dev: usize, phase: &LightPhase, log: &mut Vec<LoggedReport>) -> Result<Option<(String, Answer)>, Flow> {
        let start = self.hub.now_us();
        let deadline = start + self.opts.timeout_us;
        let mut next_remind = start + self.opts.reminder_us;
        loop {
            match self.input.poll(&step.id, attempt) {
                Some(Command::Skip) => return Err(Flow::Skip),
                Some(Command::Repeat) => return Err(Flow::Repeat),
                Some(Command::Quit) => return Err(Flow::Quit),
                Some(c @ (Command::Left | Command::Right)) => self.note_origin(c),
                Some(Command::Go) | None => {}
            }
            if let Some(text) = self.input.answer(&step.id, attempt) {
                match parse_answer(phase.ask, &text) {
                    Ok(a) => return Ok(Some((text, a))),
                    Err(message) => say!(self, "  I did not understand \"{}\": {message}.", text.trim()),
                }
            }
            if let Some(text) = self.hub.simulated_answer(dev, phase.ask, &phase.expect) {
                if let Ok(a) = parse_answer(phase.ask, &text) {
                    return Ok(Some((text, a)));
                }
            }
            self.pump(TICK_US, Some(&mut *log), &[]);
            let now = self.hub.now_us();
            if now >= deadline {
                return Ok(None);
            }
            self.remind(&mut next_remind, deadline, "your answer");
        }
    }

    /// One light step: for each question, send its commands, ask, and judge the answer.
    fn attempt_lights(&mut self, step: &Step, attempt: u32, phases: &[LightPhase]) -> Flow {
        let dev = step.device;
        if !self.hub.is_open(dev) {
            return fail_body(format!("the {} is not open", short_name(&self.descs[dev])), Vec::new(), 0);
        }
        self.step_start_us = self.hub.now_us();
        let mut log = Vec::new();
        let mut records: Vec<Json> = Vec::new();
        let mut failures: Vec<String> = Vec::new();
        let mut recorded: Vec<String> = Vec::new();
        for (i, phase) in phases.iter().enumerate() {
            let n = i + 1;
            self.input.discard_pending();
            self.hub.begin_phase(step, i as u32);
            let mut sent = Vec::new();
            let mut failed_write = None;
            for cmd in &phase.sends {
                let (ok, record) = self.send_light(dev, cmd);
                if !ok && failed_write.is_none() {
                    failed_write = Some(cmd.label.clone());
                }
                sent.push(record);
            }
            let record = Json::object().with("question_number", n).with("question", phase.text.as_str()).with("sent", Json::Arr(sent));
            if let Some(label) = failed_write {
                say!(self, "  (could not send \"{label}\"; skipping this question)");
                failures.push(format!("question {n}: sending \"{label}\" failed"));
                records.push(record.with("answer", Json::Null).with("verdict", "write failed"));
                continue;
            }
            say!(self, "  >> ({n} of {}) {}", phases.len(), phase.text);
            let answered = match self.wait_for_answer(step, attempt, dev, phase, &mut log) {
                Ok(a) => a,
                Err(flow) => return flow,
            };
            let Some((text, answer)) = answered else {
                records.push(record.with("answer", Json::Null).with("verdict", "no answer"));
                let note = format!("no answer within {} s to question {n}: {}", secs(self.opts.timeout_us), phase.text);
                let response = Json::object().with("questions", Json::Arr(records));
                return Flow::Done(Box::new(Body { outcome: Outcome::Fail, note, response, bits: Vec::new(), reports: log, reports_dropped: std::mem::take(&mut self.dropped), socket: None }));
            };
            if phase.ask == Ask::Gate && answer == Answer::No {
                records.push(record.with("answer", text.as_str()).with("verdict", "declined"));
                let response = Json::object().with("questions", Json::Arr(records));
                return Flow::Done(Box::new(Body { outcome: Outcome::Skipped, note: "declined: nothing connected to test".to_string(), response, bits: Vec::new(), reports: log, reports_dropped: std::mem::take(&mut self.dropped), socket: None }));
            }
            let (verdict, detail) = judge_light_answer(phase.ask, &phase.expect, &answer);
            match verdict {
                "fail" => failures.push(format!("question {n}: {detail}")),
                "recorded" => recorded.push(format!("question {n}: {detail}")),
                _ => {}
            }
            if verdict != "pass" {
                say!(self, "  Noted ({verdict}): {detail}");
            }
            records.push(record.with("answer", text.trim()).with("verdict", verdict).with("detail", detail));
        }
        let response = Json::object().with("questions", Json::Arr(records));
        let (outcome, note) = if failures.is_empty() {
            let extra = if recorded.is_empty() { String::new() } else { format!("; recorded: {}", recorded.join("; ")) };
            (Outcome::Pass, format!("{} question(s) answered as expected{extra}", phases.len()))
        } else {
            (Outcome::Fail, failures.join("; "))
        };
        Flow::Done(Box::new(Body { outcome, note, response, bits: Vec::new(), reports: log, reports_dropped: std::mem::take(&mut self.dropped), socket: None }))
    }

    /// Put every light this tool touched on the device back off (and the backlight intensity to 255 / 255).
    fn attempt_lights_cleanup(&mut self, step: &Step) -> Flow {
        let dev = step.device;
        let name = short_name(&self.descs[dev]);
        if !self.lights_touched.contains(&dev) {
            return Flow::Done(Box::new(Body {
                outcome: Outcome::Pass,
                note: format!("no light on the {name} was switched, so there is nothing to put back"),
                response: Json::object().with("writes", Json::Arr(Vec::new())),
                bits: Vec::new(),
                reports: Vec::new(),
                reports_dropped: 0,
                socket: None,
            }));
        }
        let model = self.descs[dev].model();
        let odd = self.toggles.get(&dev).copied().unwrap_or(0) % 2 == 1;
        let outs: Vec<u8> = self.outputs_touched.iter().filter(|(d, _)| *d == dev).map(|(_, o)| *o).collect();
        let commands = cleanup_commands(model, &outs, odd);
        let mut records = Vec::new();
        let mut failed = 0;
        for cmd in &commands {
            let (ok, record) = self.send_light(dev, cmd);
            failed += usize::from(!ok);
            records.push(record);
        }
        let response = Json::object().with("writes", Json::Arr(records)).with("writes_failed", failed);
        if failed > 0 {
            return Flow::Done(Box::new(Body { outcome: Outcome::Fail, note: format!("{failed} of {} writes to put the lights back off failed; some lights may still be on", commands.len()), response, bits: Vec::new(), reports: Vec::new(), reports_dropped: 0, socket: None }));
        }
        self.lights_touched.remove(&dev);
        self.toggles.remove(&dev);
        self.outputs_touched.retain(|(d, _)| *d != dev);
        let backlights = if model == Model::JogShuttle {
            "; backlight intensity set to 255 / 255 (the documentation gives no default; 255 is its brightest); the toggle undone if needed"
        } else {
            ""
        };
        let note = format!(
            "every light on the {name} that this tool switched is back OFF ({} writes){backlights}. The flash frequency was left as last set (the documentation gives no default)",
            commands.len()
        );
        say!(self, "  {note}");
        Flow::Done(Box::new(Body { outcome: Outcome::Pass, note, response, bits: Vec::new(), reports: Vec::new(), reports_dropped: 0, socket: None }))
    }

    // ---- driver ---------------------------------------------------------------------------------------------------------------

    fn attempt(&mut self, step: &Step, attempt: u32, total: usize) -> Flow {
        let what_next = match step.kind {
            StepKind::Hotplug => "I will say when to unplug and when to plug back in.",
            StepKind::Pair { .. } => "I will ask for several rounds.",
            StepKind::Socket { .. } => "I will ask for four things in turn: plug in, press and hold, let go, unplug.",
            StepKind::Light { .. } => "I switch the lights myself and ask what you see: type y or n (or key numbers) and press Enter.",
            StepKind::LightsCleanup => "Nothing to do for you.",
            _ => "Do it now.",
        };
        if matches!(step.kind, StepKind::Socket { .. }) && self.intro_shown.insert(step.device) {
            say!(self, "");
            say!(self, "{SOCKET_INTRO}");
        }
        if matches!(step.kind, StepKind::Light { .. }) && !self.lights_intro_shown {
            self.lights_intro_shown = true;
            say!(self, "");
            say!(self, "{LIGHTS_INTRO}");
        }
        self.announce(step, total, what_next);
        match step.kind.clone() {
            StepKind::Press { .. } | StepKind::Jog { .. } | StepKind::Shuttle { .. } | StepKind::Observe => self.attempt_press(step, attempt),
            StepKind::Pair { reps, .. } => self.attempt_pair(step, attempt, reps),
            StepKind::Hotplug => self.attempt_hotplug(step, attempt),
            StepKind::Socket { socket, .. } => self.attempt_socket(step, attempt, socket),
            StepKind::Light { phases } => self.attempt_lights(step, attempt, &phases),
            StepKind::LightsCleanup => self.attempt_lights_cleanup(step),
        }
    }

    /// After a result: wait `pause_us` (draining the devices) for `repeat`. Returns the command that interrupted it.
    fn pause(&mut self, step: &Step, attempt: u32, pause_us: i64) -> Option<Command> {
        if pause_us <= 0 {
            return None;
        }
        say!(self, "  Going on in {} s. Type  repeat  and press Enter to do this step again.", secs(pause_us));
        let end = self.hub.now_us() + pause_us;
        while self.hub.now_us() < end {
            if let Some(c @ (Command::Repeat | Command::Quit)) = self.input.poll(&step.id, attempt) {
                return Some(c);
            }
            self.pump(TICK_US, None, &[]);
        }
        None
    }

    fn skipped(&self, step: &Step, note: &str) -> StepResult {
        let now = self.hub.now_us();
        StepResult {
            step: step.clone(),
            attempts: vec![Attempt {
                outcome: Outcome::Skipped,
                note: note.to_string(),
                response: Json::object(),
                bits: Vec::new(),
                reports: Vec::new(),
                reports_dropped: 0,
                socket: None,
                started_us: now,
                ended_us: now,
            }],
        }
    }
}

/// Run the steps in order. `on_progress` is called after every step with the results so far (the caller writes the result file
/// there, so an interrupted session still leaves one).
pub fn run_session(
    hub: &mut dyn Hub,
    input: &mut dyn Input,
    out: &mut dyn Write,
    opts: &Options,
    steps: &[Step],
    on_progress: &mut dyn FnMut(&[StepResult], &str),
) -> SessionOutcome {
    let descs = hub.devices().to_vec();
    let masks = descs.iter().map(|d| comparison_mask(d.model(), d.input_len as usize, d.timestamp_offset)).collect();
    let last = vec![None; descs.len()];
    let mut e = Engine { hub, input, out, opts: opts.clone(), descs, masks, last, assigned: BTreeMap::new(), signs: BTreeMap::new(), sockets: BTreeMap::new(), intro_shown: BTreeSet::new(), counting_from: "left (assumed)".to_string(), lights_touched: BTreeSet::new(), outputs_touched: BTreeSet::new(), toggles: BTreeMap::new(), lights_intro_shown: false, step_start_us: 0, dropped: 0, quit: false };
    let total = steps.len();
    if e.opts.wait_for_go {
        let start = e.hub.now_us();
        let mut next = start;
        say!(e, "When you are ready, press Enter to begin (or type  quit  to leave).");
        loop {
            match e.input.poll("start", 1) {
                Some(Command::Go) => break,
                Some(Command::Quit) => {
                    say!(e, "Leaving without testing anything.");
                    return SessionOutcome { results: Vec::new(), not_started: true, stopped_early: true, counting_from: e.counting_from.clone() };
                }
                Some(c @ (Command::Left | Command::Right)) => e.note_origin(c),
                _ => {}
            }
            let now = e.hub.now_us();
            if now - start >= e.opts.go_timeout_us {
                say!(e, "Nobody pressed Enter within {} s, so nothing was tested and no result file was written.", secs(e.opts.go_timeout_us));
                return SessionOutcome { results: Vec::new(), not_started: true, stopped_early: false, counting_from: e.counting_from.clone() };
            }
            if now >= next {
                say!(e, "  ...waiting for you to press Enter to begin ({} s left).", secs(e.opts.go_timeout_us - (now - start)));
                next = now + 30_000_000;
            }
            e.pump(TICK_US, None, &[]);
        }
    }
    let mut results: Vec<StepResult> = Vec::new();
    for step in steps {
        let restore_lights = matches!(step.kind, StepKind::LightsCleanup) && e.lights_touched.contains(&step.device);
        if e.quit && !restore_lights {
            results.push(e.skipped(step, "stopped early at the operator's request"));
            on_progress(&results, &e.counting_from);
            continue;
        }
        if e.opts.skip_hotplug && step.kind == StepKind::Hotplug {
            say!(e, "");
            say!(e, "Step {} of {total}: {} - skipped (needs a person).", step.number, step.title);
            results.push(e.skipped(step, "needs a person at the device; not run in this mode"));
            on_progress(&results, &e.counting_from);
            continue;
        }
        let mut result = StepResult { step: step.clone(), attempts: Vec::new() };
        let mut attempt_no = 0u32;
        loop {
            attempt_no += 1;
            let started = e.hub.now_us();
            let flow = e.attempt(step, attempt_no, total);
            let ended = e.hub.now_us();
            let finish = |outcome, note: String, response, bits, reports, reports_dropped, socket| Attempt {
                outcome,
                note,
                response,
                bits,
                reports,
                reports_dropped,
                socket,
                started_us: started,
                ended_us: ended,
            };
            match flow {
                Flow::Done(b) => {
                    let (outcome, pause) = (b.outcome, if b.outcome == Outcome::Fail { e.opts.pause_fail_us } else { e.opts.pause_pass_us });
                    say!(e, "  RESULT: {} - {}", outcome.as_str().to_ascii_uppercase(), b.note);
                    result.attempts.push(finish(b.outcome, b.note, b.response, b.bits, b.reports, b.reports_dropped, b.socket));
                    match e.pause(step, attempt_no, pause) {
                        Some(Command::Repeat) => continue,
                        Some(Command::Quit) => e.quit = true,
                        _ => {}
                    }
                    break;
                }
                Flow::Skip => {
                    say!(e, "  Skipped.");
                    result.attempts.push(finish(Outcome::Skipped, "skipped at the operator's request".to_string(), Json::object(), Vec::new(), Vec::new(), 0, None));
                    break;
                }
                Flow::Repeat => {
                    say!(e, "  Starting this step over.");
                    result.attempts.push(finish(Outcome::Skipped, "restarted at the operator's request".to_string(), Json::object(), Vec::new(), Vec::new(), 0, None));
                    continue;
                }
                Flow::Quit => {
                    say!(e, "  Stopping. The steps not yet run are recorded as skipped.");
                    result.attempts.push(finish(Outcome::Skipped, "stopped at the operator's request".to_string(), Json::object(), Vec::new(), Vec::new(), 0, None));
                    e.quit = true;
                    break;
                }
            }
        }
        results.push(result);
        on_progress(&results, &e.counting_from);
    }
    let stopped_early = e.quit;
    SessionOutcome { results, not_started: false, stopped_early, counting_from: e.counting_from.clone() }
}

// ---------------------------------------------------------------------------------------------------------------------------
// The result file
// ---------------------------------------------------------------------------------------------------------------------------

/// Facts about the run that are not in the results.
pub struct Meta {
    pub mode: String,
    pub started_utc: String,
    pub finished_utc: Option<String>,
    pub qpc_frequency: i64,
    pub tool_version: String,
}

/// The key or input map derived for each device from the results of its press steps.
pub fn maps_json(devices: &[DeviceDesc], results: &[StepResult]) -> Json {
    let mut maps = Vec::new();
    for (d, desc) in devices.iter().enumerate() {
        let responses: Vec<(String, Option<Vec<BitChange>>)> = results
            .iter()
            .filter(|r| r.step.device == d && r.step.partner.is_none())
            .filter_map(|r| match &r.step.kind {
                StepKind::Press { label, .. } => {
                    let last = r.attempts.last()?;
                    Some((label.clone(), (last.outcome != Outcome::Skipped).then(|| last.bits.clone())))
                }
                _ => None,
            })
            .collect();
        if responses.is_empty() {
            continue;
        }
        let model = desc.model();
        let rows = derive_map(&responses);
        let documented = model.documented_inputs();
        let rows_json: Vec<Json> = rows
            .iter()
            .map(|r| {
                Json::object()
                    .with("control", r.label.as_str())
                    .with("status", r.status.as_str())
                    .with("byte", r.index)
                    .with("bit", r.bit)
                    .with("went_on_when_pressed", r.set)
                    .with("input", r.input)
                    .with("in_documented_set", r.input.map(|n| documented.contains(&n)))
                    .with("in_current_3x4_matrix", if model == Model::JogShuttle { r.input.map(|n| CURRENT_MATRIX_3X4.contains(&n)) } else { None })
                    .with("note", r.note.as_str())
            })
            .collect();
        let mapped: Vec<&MapRow> = rows.iter().filter(|r| r.status == MapStatus::Mapped).collect();
        let mut inputs: Vec<u32> = mapped.iter().filter_map(|r| r.input).collect();
        inputs.sort_unstable();
        maps.push(
            Json::object()
                .with("device", d)
                .with("model", model.name())
                .with("pid", format!("{:04X}", desc.pid))
                .with("controls_mapped", mapped.len())
                .with("controls_total", rows.len())
                .with("all_mapped", mapped.len() == rows.len())
                .with("all_in_documented_set", !mapped.is_empty() && mapped.iter().all(|r| r.input.is_some_and(|n| documented.contains(&n))))
                .with("inputs_found", inputs)
                .with("documented_inputs", documented.to_vec())
                .with("rows", rows_json),
        );
    }
    Json::Arr(maps)
}

/// What each Switch Interface socket showed: which bits indicate "something is plugged in" and with what polarity, which bit is the
/// button and its polarity, and which sockets share any bit with another.
pub fn socket_maps_json(devices: &[DeviceDesc], results: &[StepResult]) -> Json {
    let mut maps = Vec::new();
    for (d, desc) in devices.iter().enumerate() {
        let steps: Vec<(&StepResult, usize)> = results
            .iter()
            .filter(|r| r.step.device == d)
            .filter_map(|r| match r.step.kind {
                StepKind::Socket { socket, .. } => Some((r, socket)),
                _ => None,
            })
            .collect();
        if steps.is_empty() {
            continue;
        }
        let tested: Vec<SocketResult> = steps.iter().filter_map(|(r, _)| r.attempts.last().filter(|a| a.outcome != Outcome::Skipped).and_then(|a| a.socket.clone())).collect();
        let overlaps = socket_overlaps(&tested);
        let mut passed = 0;
        let rows: Vec<Json> = steps
            .iter()
            .map(|(r, socket)| {
                let last = r.attempts.last();
                let sr = last.filter(|a| a.outcome != Outcome::Skipped).and_then(|a| a.socket.clone());
                let status = match (r.outcome(), &sr) {
                    (Outcome::Pass, _) => "mapped",
                    (Outcome::Skipped, _) => "not tested",
                    (_, None) => "no response",
                    (_, Some(x)) if x.button.len() != 1 => "ambiguous",
                    (_, Some(x)) if !x.unique => "duplicate",
                    _ => "incomplete",
                };
                if status == "mapped" {
                    passed += 1;
                }
                let shared: Vec<usize> = sr
                    .as_ref()
                    .and_then(|x| tested.iter().position(|t| t.socket == x.socket))
                    .map(|i| overlaps[i].clone())
                    .unwrap_or_default();
                let button = sr.as_ref().and_then(|x| (x.button.len() == 1).then(|| x.button[0])).map(|b| {
                    Json::object()
                        .with("byte", b.index)
                        .with("bit", b.bit)
                        .with("input", input_number(b.index, b.bit))
                        .with("went_on_when_pressed", b.set)
                        .with("resting_level_when_plugged_in", if b.set { "OFF" } else { "ON" })
                });
                Json::object()
                    .with("socket", socket + 1)
                    .with("status", status)
                    .with("plug_bits", sr.as_ref().map_or(Json::Null, |x| bits_json(&x.plug)))
                    .with("plug_bits_note", "went_on = the bit goes ON when the plug goes in; the resting state while plugged in differs from empty")
                    .with("button_bit", button)
                    .with("button_bits_when_pressed", sr.as_ref().map_or(Json::Null, |x| bits_json(&x.button)))
                    .with("released_to_rest", sr.as_ref().map(|x| x.released_to_rest))
                    .with("unplugged_to_empty", sr.as_ref().map(|x| x.unplugged_to_empty))
                    .with("shares_bits_with_sockets", shared)
                    .with("note", last.map(|a| a.note.clone()))
            })
            .collect();
        maps.push(
            Json::object()
                .with("device", d)
                .with("model", desc.model().name())
                .with("pid", format!("{:04X}", desc.pid))
                .with("sockets_mapped", passed)
                .with("sockets_total", steps.len())
                .with("all_mapped", passed == steps.len())
                .with("rows", rows),
        );
    }
    Json::Arr(maps)
}

/// The output commands the lights part knows about, with their documented persistence and sources.
pub fn light_commands_json() -> Json {
    Json::Arr(
        lights::COMMAND_TABLE
            .iter()
            .map(|c| {
                Json::object()
                    .with("command", c.command)
                    .with("name", c.name)
                    .with("models", c.models)
                    .with("volatile", c.volatile)
                    .with("sent_by_the_lights_part", c.used)
                    .with("source", c.source)
            })
            .collect(),
    )
}

/// The whole result file.
pub fn result_json(meta: &Meta, devices: &[DeviceDesc], outcome: &SessionOutcome) -> Json {
    let devices_json: Vec<Json> = devices
        .iter()
        .enumerate()
        .map(|(i, d)| {
            Json::object()
                .with("index", i)
                .with("label", d.label())
                .with("pid", format!("{:04X}", d.pid))
                .with("product", d.product.as_str())
                .with("version", format!("{:04X}", d.version))
                .with("input_report_bytes", d.input_len)
                .with("output_report_bytes", d.output_len)
                .with("timestamp_offset", d.timestamp_offset)
                .with("polling_interval_us", d.model().polling_interval_us())
                .with("path", d.path.as_str())
        })
        .collect();
    let steps_json: Vec<Json> = outcome
        .results
        .iter()
        .map(|r| {
            let attempts: Vec<Json> = r
                .attempts
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    Json::object()
                        .with("attempt", i + 1)
                        .with("outcome", a.outcome.as_str())
                        .with("note", a.note.as_str())
                        .with("started_us", a.started_us)
                        .with("ended_us", a.ended_us)
                        .with("response", a.response.clone())
                        .with("reports_not_kept", a.reports_dropped)
                        .with("reports", Json::Arr(a.reports.iter().map(LoggedReport::json).collect()))
                })
                .collect();
            let last = r.attempts.last();
            Json::object()
                .with("number", r.step.number)
                .with("id", r.step.id.as_str())
                .with("group", r.step.group.name())
                .with("device", r.step.device)
                .with("partner_device", r.step.partner)
                .with("title", r.step.title.as_str())
                .with("prompt", r.step.prompt.as_str())
                .with("outcome", r.outcome().as_str())
                .with("note", last.map(|a| a.note.clone()))
                .with("attempts", Json::Arr(attempts))
        })
        .collect();
    let count = |o: Outcome| outcome.results.iter().filter(|r| r.outcome() == o).count();
    let (passed, failed, skipped) = (count(Outcome::Pass), count(Outcome::Fail), count(Outcome::Skipped));
    let finished = meta.finished_utc.is_some() && !outcome.stopped_early;
    Json::object()
        .with("tool", "xkprobe guide")
        .with("tool_version", meta.tool_version.as_str())
        .with("format", 1)
        .with("mode", meta.mode.as_str())
        .with("started_utc", meta.started_utc.as_str())
        .with("finished_utc", meta.finished_utc.clone())
        .with("complete", finished)
        .with("stopped_early", outcome.stopped_early)
        .with("qpc_frequency_hz", meta.qpc_frequency)
        .with("times_note", "t_us, started_us and ended_us are microseconds since the session began; device_ms is the device's own millisecond clock")
        .with("byte_note", "byte numbers count from 0; byte 0 is the report ID. input = (byte - 3) * 8 + bit")
        .with("devices", devices_json)
        .with("summary", Json::object().with("steps", outcome.results.len()).with("passed", passed).with("failed", failed).with("skipped", skipped).with("all_passed", finished && failed == 0 && skipped == 0 && passed > 0))
        .with("jog_note", JOG_KEY_OVERLAP_NOTE)
        .with("light_commands", light_commands_json())
        .with("switch_interface_sockets_counted_from", outcome.counting_from.as_str())
        .with("maps", maps_json(devices, &outcome.results))
        .with("socket_maps", socket_maps_json(devices, &outcome.results))
        .with("steps", Json::Arr(steps_json))
}

// ---------------------------------------------------------------------------------------------------------------------------
// Simulated devices
// ---------------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SimWhat {
    /// Set these bytes of the device's state, then send a report.
    Bytes(Vec<(usize, u8)>),
    /// The device is unplugged: an error entry, and the watch counts a removal.
    Unplug,
    /// The device is plugged in again: the watch counts an arrival and the device can be reopened.
    Replug,
    /// The answer to Generate Data.
    Generated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimEvent {
    pub at_us: i64,
    pub device: usize,
    pub what: SimWhat,
}

/// What a planner sees when a step begins.
pub struct SimView {
    pub now_us: i64,
    pub devices: Vec<DeviceDesc>,
    pub state: Vec<Vec<u8>>,
}

/// Decides how a simulated person answers a step: the events to schedule when it begins.
pub type Planner = Box<dyn FnMut(&Step, u32, &SimView) -> Vec<SimEvent>>;

/// The lights of one simulated device, as the person would see them.
#[derive(Clone, Debug)]
struct SimLights {
    /// By LED index: 0 and 1 the outputs, 6 green, 7 red; 0 off, 1 on, 2 flash.
    led: [u8; 8],
    /// By backlight index (bank 2 = +32).
    backlight: [u8; 64],
    intensity: [u8; 2],
    prev_intensity: [u8; 2],
    toggled_off: bool,
    freq: u8,
    prev_freq: u8,
}

impl SimLights {
    fn new() -> SimLights {
        SimLights { led: [0; 8], backlight: [0; 64], intensity: [255; 2], prev_intensity: [255; 2], toggled_off: false, freq: 128, prev_freq: 128 }
    }

    /// The keys (1 to 12) with a lit backlight in either bank.
    fn lit_keys(&self) -> BTreeSet<u8> {
        let mut lit = BTreeSet::new();
        for bank in 0..2usize {
            if self.toggled_off || self.intensity[bank] == 0 {
                continue;
            }
            for (k, &idx) in lights::JOG_SHUTTLE_KEY_INDEXES.iter().enumerate() {
                if self.backlight[idx as usize + 32 * bank] != 0 {
                    lit.insert(k as u8 + 1);
                }
            }
        }
        lit
    }
}

/// A pair of simulated devices that answer scripted "presses" in virtual time (1 tick = 1 microsecond).
pub struct SimHub {
    descs: Vec<DeviceDesc>,
    state: Vec<Vec<u8>>,
    sequence: Vec<u32>,
    open: Vec<bool>,
    replugged: Vec<bool>,
    queue: Vec<SimEvent>,
    now: i64,
    watch: u64,
    planner: Planner,
    /// Every Generate Data request made, per device.
    pub requests: Vec<u32>,
    lights: Vec<SimLights>,
    /// Every light report accepted, in order: (device, 36 bytes).
    pub writes: Vec<(usize, Vec<u8>)>,
    /// Makes the simulated backlights answer to a different index than the one sent (a device with a different index map).
    pub backlight_index_quirk: Option<fn(u8) -> u8>,
    /// Whether the simulated person answers light questions (false: nobody there, so they time out).
    pub person_answers: bool,
}

impl SimHub {
    pub fn new(descs: Vec<DeviceDesc>, planner: Planner) -> SimHub {
        let state = descs
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let mut s = vec![0u8; d.input_len as usize];
                s[1] = i as u8 + 1;
                if d.model() == Model::JogShuttle && s.len() > 5 {
                    s[5] = 0x80; // shuttle at rest
                }
                if s.len() == 37 {
                    s[36] = 1; // reboot counter
                }
                s
            })
            .collect();
        let n = descs.len();
        SimHub { descs, state, sequence: vec![0; n], open: vec![true; n], replugged: vec![false; n], queue: Vec::new(), now: 0, watch: 0, planner, requests: vec![0; n], lights: vec![SimLights::new(); n], writes: Vec::new(), backlight_index_quirk: None, person_answers: true }
    }

    /// A Jog & Shuttle (PID 0426) and an XK-12 Switch Interface (PID 04AB) with the documented layouts.
    pub fn sdk_pair() -> SimHub {
        SimHub::new(vec![sim_desc(0x0426), sim_desc(0x04AB)], sdk_planner())
    }

    fn emit(&mut self, device: usize, at: i64, generated: bool) -> (usize, ReportEntry) {
        let mut data = self.state[device].clone();
        if generated {
            data[2] |= 2;
        }
        let ts = self.descs[device].timestamp_offset;
        if ts >= 0 {
            let ms = (at / 1000) as u32 + 600_000 * (device as u32 + 1);
            data[ts as usize..ts as usize + 4].copy_from_slice(&ms.to_be_bytes());
        }
        let entry = ReportEntry::report(at, self.sequence[device], false, &data, ts);
        self.sequence[device] += 1;
        (device, entry)
    }

    fn apply(&mut self, ev: SimEvent) -> Vec<(usize, ReportEntry)> {
        let d = ev.device;
        match ev.what {
            SimWhat::Bytes(bytes) if self.open[d] => {
                for (i, v) in bytes {
                    self.state[d][i] = v;
                }
                vec![self.emit(d, ev.at_us, false)]
            }
            SimWhat::Generated if self.open[d] => vec![self.emit(d, ev.at_us, true)],
            SimWhat::Unplug if self.open[d] => {
                self.open[d] = false;
                self.replugged[d] = false;
                self.watch += 1;
                let entry = ReportEntry::error(ev.at_us, self.sequence[d], XKHID_E_DEVICE_REMOVED, 1167);
                self.sequence[d] += 1;
                vec![(d, entry)]
            }
            SimWhat::Replug => {
                self.watch += 1;
                self.replugged[d] = true;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

/// A simulated device description for a PID.
pub fn sim_desc(pid: u16) -> DeviceDesc {
    let model = model_for_pid(pid);
    DeviceDesc {
        pid,
        product: model.name().to_string(),
        version: 0,
        input_len: if model == Model::JogShuttle { 33 } else { 37 },
        output_len: 36,
        timestamp_offset: timestamp_offset_for_pid(pid),
        path: format!("simulated-{pid:04X}"),
    }
}

impl Hub for SimHub {
    fn devices(&self) -> &[DeviceDesc] {
        &self.descs
    }

    fn to_us(&self, qpc: i64) -> i64 {
        qpc
    }

    fn now_us(&self) -> i64 {
        self.now
    }

    fn poll(&mut self, max_wait_us: i64) -> Vec<(usize, ReportEntry)> {
        let limit = self.now + max_wait_us;
        self.queue.sort_by_key(|e| e.at_us);
        let Some(first) = self.queue.first().map(|e| e.at_us).filter(|&t| t <= limit) else {
            self.now = limit;
            return Vec::new();
        };
        self.now = self.now.max(first);
        let mut out = Vec::new();
        while self.queue.first().is_some_and(|e| e.at_us <= first) {
            let ev = self.queue.remove(0);
            out.extend(self.apply(ev));
        }
        out
    }

    fn request_report(&mut self, device: usize) -> Result<(), String> {
        if !self.open[device] {
            return Err("not open".to_string());
        }
        self.requests[device] += 1;
        let at = self.now + 3000;
        self.queue.push(SimEvent { at_us: at, device, what: SimWhat::Generated });
        Ok(())
    }

    fn begin_phase(&mut self, step: &Step, phase: u32) {
        // Whatever a previous, abandoned attempt still had pending is settled silently, so the simulated person starts fresh.
        self.queue.sort_by_key(|e| e.at_us);
        for ev in std::mem::take(&mut self.queue) {
            if let (SimWhat::Bytes(bytes), true) = (ev.what, self.open[ev.device]) {
                for (i, v) in bytes {
                    self.state[ev.device][i] = v;
                }
            }
        }
        let view = SimView { now_us: self.now, devices: self.descs.clone(), state: self.state.clone() };
        let events = (self.planner)(step, phase, &view);
        self.queue.extend(events);
    }

    fn is_open(&self, device: usize) -> bool {
        self.open[device]
    }

    fn reopen(&mut self, device: usize) -> Result<(), String> {
        if self.replugged[device] || self.open[device] {
            self.open[device] = true;
            Ok(())
        } else {
            Err("not found".to_string())
        }
    }

    fn watch_generation(&self) -> u64 {
        self.watch
    }

    fn send_light(&mut self, device: usize, report: &[u8]) -> Result<(), String> {
        if !self.open[device] {
            return Err("not open".to_string());
        }
        lights::validate_light_report(light_model(self.descs[device].model()), report)?;
        self.writes.push((device, report.to_vec()));
        let quirk = self.backlight_index_quirk;
        let l = &mut self.lights[device];
        match report[1] {
            lights::CMD_SET_LED => l.led[report[2] as usize & 7] = report[3],
            lights::CMD_SET_LEDS => {
                l.led[6] = u8::from(report[2] & lights::LEDS_GREEN != 0);
                l.led[7] = u8::from(report[2] & lights::LEDS_RED != 0);
            }
            lights::CMD_SET_FLASH_FREQUENCY => {
                l.prev_freq = l.freq;
                l.freq = report[2];
            }
            lights::CMD_SET_BACKLIGHT => {
                let idx = quirk.map_or(report[2], |q| q(report[2]));
                l.backlight[idx as usize % 64] = report[3];
            }
            lights::CMD_SET_BACKLIGHT_ROWS => {
                // The "rows" of the documentation are taken to be the key columns of Figure 1 (the bytes D1..D4).
                let bank = report[2] as usize;
                for (k, &idx) in lights::JOG_SHUTTLE_KEY_INDEXES.iter().enumerate() {
                    let column = k % 4;
                    l.backlight[idx as usize + 32 * bank] = u8::from(report[3] >> column & 1 == 1);
                }
            }
            lights::CMD_TOGGLE_BACKLIGHTS => l.toggled_off = !l.toggled_off,
            lights::CMD_SET_BACKLIGHT_INTENSITY => {
                l.prev_intensity = l.intensity;
                l.intensity = [report[2], report[3]];
            }
            _ => {}
        }
        Ok(())
    }

    fn simulated_answer(&mut self, device: usize, ask: Ask, expect: &Expect) -> Option<String> {
        if !self.person_answers {
            return None;
        }
        let l = &self.lights[device];
        let keys = |s: &BTreeSet<u8>| if s.is_empty() { "none".to_string() } else { s.iter().map(|k| k.to_string()).collect::<Vec<_>>().join(" ") };
        if matches!(ask, Ask::Keys | Ask::Observe) {
            return Some(keys(&l.lit_keys()));
        }
        let truth = match expect {
            Expect::Nothing => true,
            Expect::Led { led, state } => l.led[*led as usize & 7] == *state as u8,
            Expect::Leds { green, red } => (l.led[6] == 1) == *green && (l.led[7] == 1) == *red,
            Expect::FlashFaster => l.freq < l.prev_freq,
            Expect::KeysLit(want) => &l.lit_keys() == want,
            Expect::AllBacklights { on } => (l.lit_keys().len() == 12) == *on,
            Expect::Dimmer { bank } => l.intensity[*bank as usize] < l.prev_intensity[*bank as usize],
            Expect::ToggleState { off } => l.toggled_off == *off,
            Expect::KeyFlashing { bank, key } => {
                let idx = lights::JOG_SHUTTLE_KEY_INDEXES[*key as usize - 1] as usize + 32 * *bank as usize;
                l.backlight[idx] == 2 && l.lit_keys() == BTreeSet::from([*key])
            }
            Expect::Rows { .. } => true,
            Expect::Output { out, on } => (l.led[*out as usize & 7] == 1) == *on,
        };
        Some(if truth { "y" } else { "n" }.to_string())
    }
}

/// The bytes of a Switch Interface report that change when a mono-plug push button is in socket `socket` (0-based) in the given state.
/// As measured on a test machine: plugging in sets bit 2n (n = socket number from 0), pressing the button sets bit 2n+1, both at rest OFF.
fn socket_bytes(st: &[u8], socket: usize, plugged: bool, pressed: bool) -> Vec<(usize, u8)> {
    let mut out: Vec<(usize, u8)> = Vec::new();
    for (k, level) in [(2 * socket, plugged), (2 * socket + 1, plugged && pressed)] {
        let (i, b) = (3 + k / 8, (k % 8) as u8);
        let cur = out.iter().find(|(x, _)| *x == i).map_or(st[i], |(_, v)| *v);
        out.retain(|(x, _)| *x != i);
        out.push((i, if level { cur | 1 << b } else { cur & !(1 << b) }));
    }
    out
}

fn event(at_us: i64, device: usize, bytes: Vec<(usize, u8)>) -> SimEvent {
    SimEvent { at_us, device, what: SimWhat::Bytes(bytes) }
}

/// A simulated person who follows the P.I. Engineering documentation exactly.
pub fn sdk_planner() -> Planner {
    Box::new(|step, phase, view| {
        let t0 = view.now_us;
        let dev = step.device;
        let model = view.devices[dev].model();
        let st = &view.state[dev];
        let flip = |slot: usize| bit_for_slot(model, slot).map(|(i, b)| (i, st[i] ^ (1 << b), st[i]));
        match &step.kind {
            StepKind::Press { slot, .. } => {
                let (i, on, off) = flip(*slot).expect("a slot with a documented bit");
                vec![event(t0 + 600_000, dev, vec![(i, on)]), event(t0 + 900_000, dev, vec![(i, off)])]
            }
            StepKind::Jog { clockwise } => {
                // As measured: one report per detent (byte 7 = +1 or 0xFF and the direction's pulse flag), and the resting state
                // about 32 ms later. Clockwise is positive.
                let (side, value) = if *clockwise { (3usize, 1u8) } else { (4, 0xFF) };
                (0..8)
                    .flat_map(|k| {
                        let at = t0 + 600_000 + k * 150_000;
                        [
                            event(at, dev, vec![(7, value), (side, st[side] | 0x80)]),
                            event(at + 32_000, dev, vec![(7, st[7]), (side, st[side])]),
                        ]
                    })
                    .collect()
            }
            StepKind::Shuttle { clockwise } => {
                // As measured: the position is held in byte 8 (+n / -n) with one documented bit of D1..D4 per position, and the
                // "at rest" bit (byte 5 bit 7) is clear. Positions 1 to 3 are visited, 400 ms each, then the ring springs back.
                let mut ev = Vec::new();
                for n in 1..=3usize {
                    let mut state = st.clone();
                    state[5] &= !0x80;
                    let bit = if *clockwise { 3 + (n - 1) / 4 } else { 5 + (n - 1) / 4 };
                    state[3 + (n - 1) % 4] |= 1 << bit;
                    state[8] = if *clockwise { n as u8 } else { 0u8.wrapping_sub(n as u8) };
                    let bytes = vec![(3, state[3]), (4, state[4]), (5, state[5]), (6, state[6]), (8, state[8])];
                    ev.push(event(t0 + 500_000 + n as i64 * 400_000, dev, bytes));
                }
                ev.push(event(t0 + 500_000 + 4 * 400_000, dev, vec![(3, st[3]), (4, st[4]), (5, st[5]), (6, st[6]), (8, st[8])]));
                ev
            }
            StepKind::Socket { socket, .. } => {
                // As measured: plugging a push button on a mono plug into socket n sets bit 2n; pressing the button sets bit 2n+1;
                // letting go clears it; unplugging clears both.
                let (plugged, pressed) = match phase {
                    0 => (true, false),
                    1 => (true, true),
                    2 => (true, false),
                    _ => (false, false),
                };
                vec![event(t0 + 600_000, dev, socket_bytes(st, *socket, plugged, pressed))]
            }
            StepKind::Pair { slots, plug_socket, .. } if phase == PLUG_PHASE => {
                let _ = slots;
                let si = [Some(dev), step.partner].into_iter().flatten().find(|&d| view.devices[d].model() == Model::SwitchInterface12);
                si.map(|d| vec![event(t0 + 600_000, d, socket_bytes(&view.state[d], plug_socket.unwrap_or(0), true, false))]).unwrap_or_default()
            }
            StepKind::Pair { slots, .. } => {
                let gap = [0i64, 2000, 0, 7000, 500][(phase % 5) as usize];
                let at = t0 + 600_000;
                if let Some(partner) = step.partner {
                    let pm = view.devices[partner].model();
                    let (pi, pbit) = bit_for_slot(pm, 0).unwrap();
                    let pst = &view.state[partner];
                    let (i, b) = bit_for_slot(model, slots.0).unwrap();
                    vec![
                        event(at, dev, vec![(i, st[i] ^ (1 << b))]),
                        event(at + gap, partner, vec![(pi, pst[pi] ^ (1 << pbit))]),
                        event(at + 300_000, dev, vec![(i, st[i])]),
                        event(at + 300_000 + gap, partner, vec![(pi, pst[pi])]),
                    ]
                } else {
                    let (ia, ba) = bit_for_slot(model, slots.0).unwrap();
                    let (ib, bb) = bit_for_slot(model, slots.1).unwrap();
                    let mut base = st.clone();
                    let mut ev = Vec::new();
                    if gap == 0 {
                        let mut both = st.clone();
                        both[ia] ^= 1 << ba;
                        both[ib] ^= 1 << bb;
                        ev.push(event(at, dev, vec![(ia, both[ia]), (ib, both[ib])]));
                    } else {
                        base[ia] ^= 1 << ba;
                        ev.push(event(at, dev, vec![(ia, base[ia])]));
                        base[ib] ^= 1 << bb;
                        ev.push(event(at + gap, dev, vec![(ib, base[ib])]));
                    }
                    ev.push(event(at + 300_000, dev, vec![(ia, st[ia]), (ib, st[ib])]));
                    ev
                }
            }
            StepKind::Observe | StepKind::Light { .. } | StepKind::LightsCleanup => Vec::new(),
            StepKind::Hotplug => vec![SimEvent { at_us: t0 + 800_000, device: dev, what: if phase == 0 { SimWhat::Unplug } else { SimWhat::Replug } }],
        }
    })
}

// ---------------------------------------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A recorded J&S report (index 0 first): unit 2, data type 0, plus the given D1..D4, jog and shuttle bytes and a timestamp.
    fn js_report(d: [u8; 4], jog: u8, shuttle: u8, ms: u32) -> Vec<u8> {
        let mut r = vec![0u8; 33];
        r[1] = 2;
        r[3..7].copy_from_slice(&d);
        r[7] = jog;
        r[8] = shuttle;
        r[9..13].copy_from_slice(&ms.to_be_bytes());
        r
    }

    #[test]
    fn diff_reports_into_changed_bits_ignoring_timestamp_and_data_type_bit() {
        let mask = comparison_mask(Model::JogShuttle, 33, 9);
        assert_eq!(mask[0..3], [0, 0, 0xFD]);
        assert_eq!(mask[9..13], [0, 0, 0, 0]);
        let a = js_report([0, 0, 0x80, 0], 0, 0, 859_326);
        let mut b = js_report([0x01, 0, 0x80, 0], 0, 0, 859_820);
        b[2] = 2; // generated
        let changes = diff_bits(&mask, &a, &b);
        assert_eq!(changes, vec![BitChange { index: 3, bit: 0, set: true }]);
        assert_eq!(describe_bits(&changes), "byte 3 bit 0 went ON");
        assert_eq!(diff_bytes(&mask, &a, &b), vec![(3, 0, 1)]);
        // Both directions, in byte order.
        let c = js_report([0x01, 0, 0, 0], 0, 0, 0);
        let both = diff_bits(&mask, &js_report([0, 0, 0x80, 0], 0, 0, 0), &c);
        assert_eq!(both, vec![BitChange { index: 3, bit: 0, set: true }, BitChange { index: 5, bit: 7, set: false }]);
        // Identical apart from the timestamp: nothing.
        assert!(diff_bits(&mask, &a, &js_report([0, 0, 0x80, 0], 0, 0, 5)).is_empty());
    }

    #[test]
    fn switch_interface_mask_ignores_timestamp_and_reboot_counter() {
        let mask = comparison_mask(Model::SwitchInterface12, 37, 32);
        assert_eq!(&mask[32..37], &[0, 0, 0, 0, 0]);
        assert_eq!(mask[3], 0xFF);
        assert!(is_state_report(&[0, 1, 2]) && !is_state_report(&[0, 1, 214]) && !is_state_report(&[0, 1]));
    }

    #[test]
    fn tracker_follows_a_press_and_release() {
        let mask = comparison_mask(Model::JogShuttle, 33, 9);
        let rest = js_report([0, 0, 0x80, 0], 0, 0, 100);
        let mut t = Tracker::new(rest.clone(), mask);
        assert!(t.feed(&js_report([0, 0, 0x80, 0], 0, 0, 200)).is_empty());
        assert!(!t.seen_active);
        let down = t.feed(&js_report([0x04, 0, 0x80, 0], 0, 0, 300));
        assert_eq!(down, vec![BitChange { index: 3, bit: 2, set: true }]);
        assert!(t.is_active() && t.seen_active);
        assert!(t.feed(&js_report([0, 0, 0x80, 0], 0, 0, 400)).is_empty());
        assert!(!t.is_active() && t.seen_active);
        assert_eq!(t.bits(), vec![BitChange { index: 3, bit: 2, set: true }]);
        // Shuttle values are collected per byte.
        let mut s = Tracker::new(rest, comparison_mask(Model::JogShuttle, 33, 9));
        for v in [1, 2, 3, 0] {
            s.feed(&js_report([0, 0, 0x80, 0], 0, v, 0));
        }
        assert_eq!(s.bytes[&8], BTreeSet::from([1, 2, 3]));
    }

    fn row(label: &str, bits: &[(usize, u8, bool)]) -> (String, Option<Vec<BitChange>>) {
        (label.to_string(), Some(bits.iter().map(|&(index, bit, set)| BitChange { index, bit, set }).collect()))
    }

    #[test]
    fn map_is_derived_from_prompt_and_response_pairs() {
        let rows = derive_map(&[
            row("key 1", &[(3, 0, true)]),
            row("key 2", &[(3, 1, true)]),
            row("key 3", &[(3, 1, true)]),
            row("key 4", &[]),
            row("key 5", &[(3, 3, true), (4, 0, true)]),
            ("key 6".to_string(), None),
            row("key 7", &[(6, 2, true)]),
        ]);
        let status: Vec<MapStatus> = rows.iter().map(|r| r.status).collect();
        assert_eq!(
            status,
            [MapStatus::Mapped, MapStatus::Mapped, MapStatus::Duplicate, MapStatus::NoResponse, MapStatus::Ambiguous, MapStatus::NotTested, MapStatus::Mapped]
        );
        assert_eq!(rows[0].input, Some(0));
        assert_eq!(rows[1].input, Some(1));
        assert_eq!(rows[2].note, "the same bit as key 2");
        assert!(rows[4].note.contains("byte 3 bit 3 went ON"));
        assert_eq!(rows[6].input, Some(26));
        assert_eq!(input_number(2, 0), None);
    }

    fn pair_devices() -> Vec<DeviceDesc> {
        vec![sim_desc(0x04AB), sim_desc(0x0426)]
    }

    #[test]
    fn plan_has_the_documented_steps_in_order() {
        let steps = plan_steps(&pair_devices(), &Selection::all(), 5);
        // Jog & Shuttle first: 12 keys, 2 jog, 2 shuttle; then the switch interface: 6 sockets; 1 timing + 1 cross; 2 hot-plug.
        assert_eq!(steps.len(), 12 + 4 + 6 + 2 + 2 + 16, "26 control steps plus 11 + 5 light steps");
        assert_eq!(steps.iter().map(|s| s.number).collect::<Vec<_>>(), (1..=steps.len()).collect::<Vec<_>>());
        let ids: BTreeSet<&str> = steps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids.len(), steps.len(), "ids are unique");
        assert_eq!(steps[0].id, "js1-key-01");
        assert_eq!(steps[0].device, 1);
        assert!(steps[0].prompt.contains("Press key 1 of 12 (the top-left key)"));
        assert_eq!(steps[12].id, "js1-jog-cw");
        assert_eq!(steps[16].id, "si0-socket-1");
        assert!(steps[16].prompt.contains("Socket 1 of 6"));
        assert_eq!(steps.iter().filter(|s| s.group == Group::Timing).count(), 2);
        assert_eq!(steps.last().unwrap().kind, StepKind::Hotplug);
        let cross = steps.iter().find(|s| s.id == "cross-timing").unwrap();
        assert_eq!((cross.device, cross.partner), (1, Some(0)));
        // Selecting a part leaves only that part.
        let only = plan_steps(&pair_devices(), &Selection::parse("hotplug").unwrap(), 5);
        assert_eq!(only.len(), 2);
        let one = plan_steps(&pair_devices()[..1], &Selection::all(), 3);
        assert_eq!(one.len(), 6 + 1 + 5);
        assert!(Selection::parse("bogus").is_err() && Selection::parse("").is_err());
        assert_eq!(device_labels(&[sim_desc(0x0426), sim_desc(0x0426)]), ["XK-12 Jog & Shuttle (PID 0426), unit 1 of 2", "XK-12 Jog & Shuttle (PID 0426), unit 2 of 2"]);
    }

    #[test]
    fn commands_are_words_so_stray_typing_is_ignored() {
        assert_eq!(parse_command("skip"), Some(Command::Skip));
        assert_eq!(parse_command("  Repeat "), Some(Command::Repeat));
        assert_eq!(parse_command("quit"), Some(Command::Quit));
        assert_eq!(parse_command(""), Some(Command::Go));
        assert_eq!(parse_command("s"), None);
        assert_eq!(parse_command("q"), None);
        assert_eq!(parse_command("abc123"), None);
        let mut s = ScriptedInput::parse("# comment\njs1-key-02 skip\njs1-key-03 2 repeat\n").unwrap();
        assert_eq!(s.poll("js1-key-02", 1), Some(Command::Skip));
        assert_eq!(s.poll("js1-key-02", 1), None);
        assert_eq!(s.poll("js1-key-03", 1), None);
        assert_eq!(s.poll("js1-key-03", 2), Some(Command::Repeat));
        assert!(ScriptedInput::parse("justanid").is_err());
    }

    #[test]
    fn round_analysis_groups_reports_by_polling_interval() {
        let f = |device, bit, t_us, ms, sequence| FirstSeen { device, key: (3, bit), t_us, device_ms: ms, sequence };
        // One report with both keys.
        let same = analyze_round(&[f(0, 0, 1000, Some(500), 7), f(0, 2, 1000, Some(500), 7)], false, Some(8000)).unwrap();
        assert!(same.same_report && same.gap_us == 0 && same.within_polling_interval == Some(true) && same.device_gap_ms == Some(0));
        // Two reports 2 ms apart on a device polled every 8 ms: separate, but within the interval.
        let near = analyze_round(&[f(0, 2, 3000, Some(502), 8), f(0, 0, 1000, Some(500), 7)], false, Some(8000)).unwrap();
        assert!(!near.same_report);
        assert_eq!((near.gap_us, near.device_gap_ms, near.within_polling_interval), (2000, Some(2), Some(true)));
        let far = analyze_round(&[f(0, 0, 1000, Some(500), 7), f(0, 2, 41_000, Some(540), 8)], false, Some(8000)).unwrap();
        assert_eq!(far.within_polling_interval, Some(false));
        // Across two devices the first report of each is compared; there is no device-clock gap.
        let cross = analyze_round(&[f(0, 0, 10_000, None, 1), f(0, 1, 11_000, None, 2), f(1, 0, 9_400, None, 1)], true, Some(8000)).unwrap();
        assert_eq!((cross.first.device, cross.second.device, cross.gap_us), (1, 0, 600));
        assert_eq!((cross.device_gap_ms, cross.within_polling_interval), (None, None));
        assert!(analyze_round(&[f(0, 0, 1, None, 1)], false, None).is_none());
        assert!(analyze_round(&[f(0, 0, 1, None, 1), f(0, 1, 2, None, 2)], true, None).is_none());
    }

    // ---- JSON and dates -------------------------------------------------------------------------------------------------------

    /// A small strict JSON parser, to prove the rendered text is well formed.
    fn parse(text: &str) -> Result<Json, String> {
        struct P<'a> {
            b: &'a [u8],
            i: usize,
        }
        impl P<'_> {
            fn ws(&mut self) {
                while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
                    self.i += 1;
                }
            }
            fn eat(&mut self, c: u8) -> Result<(), String> {
                self.ws();
                if self.b.get(self.i) == Some(&c) {
                    self.i += 1;
                    Ok(())
                } else {
                    Err(format!("expected {} at {}", c as char, self.i))
                }
            }
            fn value(&mut self) -> Result<Json, String> {
                self.ws();
                match self.b.get(self.i).copied().ok_or("unexpected end")? {
                    b'{' => {
                        self.i += 1;
                        let mut fields = Vec::new();
                        self.ws();
                        if self.b[self.i] == b'}' {
                            self.i += 1;
                            return Ok(Json::Obj(fields));
                        }
                        loop {
                            self.ws();
                            let Json::Str(k) = self.value()? else { return Err("key".into()) };
                            self.eat(b':')?;
                            fields.push((k, self.value()?));
                            self.ws();
                            match self.b[self.i] {
                                b',' => self.i += 1,
                                b'}' => {
                                    self.i += 1;
                                    return Ok(Json::Obj(fields));
                                }
                                _ => return Err(format!("bad object at {}", self.i)),
                            }
                        }
                    }
                    b'[' => {
                        self.i += 1;
                        let mut items = Vec::new();
                        self.ws();
                        if self.b[self.i] == b']' {
                            self.i += 1;
                            return Ok(Json::Arr(items));
                        }
                        loop {
                            items.push(self.value()?);
                            self.ws();
                            match self.b[self.i] {
                                b',' => self.i += 1,
                                b']' => {
                                    self.i += 1;
                                    return Ok(Json::Arr(items));
                                }
                                _ => return Err(format!("bad array at {}", self.i)),
                            }
                        }
                    }
                    b'"' => {
                        self.i += 1;
                        let mut s = String::new();
                        loop {
                            match self.b[self.i] {
                                b'"' => {
                                    self.i += 1;
                                    return Ok(Json::Str(s));
                                }
                                b'\\' => {
                                    self.i += 1;
                                    match self.b[self.i] {
                                        b'n' => s.push('\n'),
                                        b'r' => s.push('\r'),
                                        b't' => s.push('\t'),
                                        b'"' => s.push('"'),
                                        b'\\' => s.push('\\'),
                                        b'u' => {
                                            let h = std::str::from_utf8(&self.b[self.i + 1..self.i + 5]).unwrap();
                                            s.push(char::from_u32(u32::from_str_radix(h, 16).unwrap()).unwrap());
                                            self.i += 4;
                                        }
                                        c => return Err(format!("bad escape {}", c as char)),
                                    }
                                    self.i += 1;
                                }
                                c if c < 0x20 => return Err("raw control character".into()),
                                _ => {
                                    let rest = std::str::from_utf8(&self.b[self.i..]).unwrap();
                                    let ch = rest.chars().next().unwrap();
                                    s.push(ch);
                                    self.i += ch.len_utf8();
                                }
                            }
                        }
                    }
                    b't' if self.b[self.i..].starts_with(b"true") => {
                        self.i += 4;
                        Ok(Json::Bool(true))
                    }
                    b'f' if self.b[self.i..].starts_with(b"false") => {
                        self.i += 5;
                        Ok(Json::Bool(false))
                    }
                    b'n' if self.b[self.i..].starts_with(b"null") => {
                        self.i += 4;
                        Ok(Json::Null)
                    }
                    _ => {
                        let start = self.i;
                        while self.i < self.b.len() && matches!(self.b[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                            self.i += 1;
                        }
                        let t = std::str::from_utf8(&self.b[start..self.i]).unwrap();
                        if let Ok(i) = t.parse::<i64>() {
                            Ok(Json::Int(i))
                        } else {
                            t.parse::<f64>().map(Json::Num).map_err(|_| format!("bad number \"{t}\" at {start}"))
                        }
                    }
                }
            }
        }
        let mut p = P { b: text.as_bytes(), i: 0 };
        let v = p.value()?;
        p.ws();
        if p.i == p.b.len() { Ok(v) } else { Err(format!("trailing text at {}", p.i)) }
    }

    fn field<'a>(v: &'a Json, key: &str) -> &'a Json {
        match v {
            Json::Obj(f) => f.iter().find(|(k, _)| k == key).map(|(_, v)| v).unwrap_or_else(|| panic!("no field {key}")),
            _ => panic!("not an object"),
        }
    }

    #[test]
    fn json_renders_well_formed_text_and_escapes() {
        let j = Json::object()
            .with("text", "quote \" backslash \\ newline \n tab \t bell \u{7} caf\u{e9}")
            .with("n", -5)
            .with("f", 1.23456)
            .with("nan", f64::NAN)
            .with("none", Option::<i32>::None)
            .with("list", vec![1, 2, 3])
            .with("empty", Json::Arr(vec![]))
            .with("rows", Json::Arr(vec![Json::object().with("a", 1).with("b", "x"), Json::object().with("a", Json::Arr(vec![]))]))
            .with("nested", Json::object().with("deep", Json::object().with("k", true)));
        let text = j.render();
        let back = parse(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert_eq!(field(&back, "text"), &Json::Str("quote \" backslash \\ newline \n tab \t bell \u{7} caf\u{e9}".into()));
        assert_eq!(field(&back, "f"), &Json::Num(1.235));
        assert_eq!(field(&back, "nan"), &Json::Null);
        assert!(text.contains("{\"a\": 1, \"b\": \"x\"}"), "flat rows take one line:\n{text}");
        assert!(parse("{\"a\": 1,}").is_err(), "the checker itself is strict");
    }

    #[test]
    fn dates_are_formatted_in_utc() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(iso_utc(1_709_164_800 + 86_399), "2024-02-29T23:59:59Z");
        assert_eq!(result_file_name(1_700_000_000), "xkprobe-guide-20231114T221320Z.json");
    }

    // ---- whole sessions against simulated devices -----------------------------------------------------------------------------

    fn meta() -> Meta {
        Meta { mode: "test".into(), started_utc: iso_utc(0), finished_utc: Some(iso_utc(1)), qpc_frequency: 1_000_000, tool_version: "test".into() }
    }

    fn run(hub: &mut SimHub, input: &mut dyn Input, opts: &Options, sel: &Selection) -> (SessionOutcome, String, Vec<Step>) {
        let steps = plan_steps(hub.devices(), sel, 5);
        let mut out = Vec::new();
        let mut progress = 0;
        let outcome = run_session(hub, input, &mut out, opts, &steps, &mut |r, _| progress = r.len());
        assert_eq!(progress, outcome.results.len(), "progress is reported after every step");
        (outcome, String::from_utf8(out).unwrap(), steps)
    }

    fn opts() -> Options {
        Options::unattended(5_000_000)
    }

    #[test]
    fn a_simulated_session_following_the_documentation_passes_every_step() {
        let mut hub = SimHub::sdk_pair();
        let (outcome, screen, steps) = run(&mut hub, &mut ScriptedInput::default(), &opts(), &Selection::all());
        assert_eq!(outcome.results.len(), steps.len());
        for r in &outcome.results {
            assert_eq!(r.outcome(), Outcome::Pass, "{}: {}", r.step.id, r.attempts.last().unwrap().note);
        }
        assert!(screen.contains("Step 1 of 42: XK-12 Jog & Shuttle (PID 0426) - key 1"));
        assert!(screen.contains("RESULT: PASS - key 1 = byte 3 bit 0 (input number 0), as documented"));
        assert!(screen.contains("byte 3 bit 0 went ON (byte 3: 00 -> 01)"), "{screen}");
        assert!(screen.contains("Stage 1 of 2: UNPLUG"));

        let doc = result_json(&meta(), hub.devices(), &outcome);
        let text = doc.render();
        let back = parse(&text).unwrap_or_else(|e| panic!("{e}"));
        let summary = field(&back, "summary");
        assert_eq!((field(summary, "passed"), field(summary, "all_passed")), (&Json::Int(42), &Json::Bool(true)));
        let Json::Arr(maps) = field(&back, "maps") else { panic!() };
        assert_eq!(maps.len(), 1, "key map for the Jog & Shuttle; the Switch Interface has a socket map");
        for m in maps {
            assert_eq!(field(m, "all_mapped"), &Json::Bool(true));
            assert_eq!(field(m, "all_in_documented_set"), &Json::Bool(true));
        }
        let Json::Arr(socket_maps) = field(&back, "socket_maps") else { panic!() };
        assert_eq!((socket_maps.len(), field(&socket_maps[0], "all_mapped")), (1, &Json::Bool(true)));
        let Json::Arr(step_list) = field(&back, "steps") else { panic!() };
        let Json::Arr(attempts) = field(&step_list[0], "attempts") else { panic!() };
        let Json::Arr(reports) = field(&attempts[0], "reports") else { panic!() };
        assert!(reports.len() >= 3, "baseline answer, press, release");
        assert_eq!(field(&reports[0], "generated"), &Json::Bool(true));
        assert!(matches!(field(&reports[1], "device_ms"), Json::Int(_)));
        // Hot-plug, jog and shuttle responses are recorded.
        let hot = step_list.iter().find(|s| field(s, "id") == &Json::Str("js0-hotplug".into()) || field(s, "id") == &Json::Str("si0-hotplug".into())).unwrap();
        let Json::Arr(a) = field(hot, "attempts") else { panic!() };
        assert_eq!(field(field(&a[0], "response"), "error_entries"), &Json::Int(1));
        let timing = step_list.iter().find(|s| field(s, "id") == &Json::Str("js0-timing".into())).unwrap();
        let Json::Arr(a) = field(timing, "attempts") else { panic!() };
        let resp = field(&a[0], "response");
        assert_eq!((field(resp, "same_report"), field(resp, "separate_reports")), (&Json::Int(2), &Json::Int(3)));
        assert_eq!(field(resp, "polling_interval_us"), &Json::Int(8000));
        // Only Generate Data was ever asked of the devices: one baseline per prepare, nothing else exists in the Hub trait.
        assert!(hub.requests.iter().all(|&n| n > 0));
    }

    #[test]
    fn a_session_where_nobody_does_anything_times_out_cleanly_and_says_what_it_waits_for() {
        let mut hub = SimHub::new(pair_devices(), Box::new(|_, _, _| Vec::new()));
        hub.person_answers = false;
        let (outcome, screen, steps) = run(&mut hub, &mut ScriptedInput::default(), &Options::unattended(2_000_000), &Selection::all());
        assert_eq!(outcome.results.len(), steps.len());
        // Everything fails except putting the lights back off, which needs no person.
        assert!(outcome.results.iter().filter(|r| r.step.kind != StepKind::LightsCleanup).all(|r| r.outcome() == Outcome::Fail));
        assert!(screen.contains("nothing arrived from the Jog & Shuttle within 2 s"));
        assert!(screen.contains("not unplugged within"), "{screen}");
        assert!(screen.contains("Waiting up to 2 s. Type  skip  and press Enter to skip this step"));
        let text = result_json(&meta(), hub.devices(), &outcome).render();
        let back = parse(&text).unwrap();
        assert_eq!(field(field(&back, "summary"), "all_passed"), &Json::Bool(false));
        assert_eq!(field(field(&back, "summary"), "failed"), &Json::Int(40));
    }

    #[test]
    fn reminders_are_printed_while_waiting() {
        let mut hub = SimHub::new(vec![sim_desc(0x0426)], Box::new(|_, _, _| Vec::new()));
        let mut o = Options::unattended(25_000_000);
        o.reminder_us = 10_000_000;
        let (_, screen, _) = run(&mut hub, &mut ScriptedInput::default(), &o, &Selection::parse("keys").unwrap());
        assert!(screen.contains("...still waiting for key 1 to be pressed (15 s left)."), "{screen}");
    }

    #[test]
    fn skip_and_repeat_are_honored_and_recorded() {
        let mut hub = SimHub::sdk_pair();
        let mut input = ScriptedInput::parse("js0-key-01 skip\njs0-key-02 repeat\n").unwrap();
        let (outcome, screen, _) = run(&mut hub, &mut input, &opts(), &Selection::parse("keys").unwrap());
        assert_eq!(outcome.results[0].outcome(), Outcome::Skipped);
        assert_eq!(outcome.results[1].attempts.len(), 2);
        assert_eq!(outcome.results[1].attempts[0].outcome, Outcome::Skipped);
        assert_eq!(outcome.results[1].outcome(), Outcome::Pass);
        assert!(screen.contains("Skipped.") && screen.contains("Starting this step over."));
        let text = result_json(&meta(), hub.devices(), &outcome).render();
        let back = parse(&text).unwrap();
        let Json::Arr(maps) = field(&back, "maps") else { panic!() };
        assert_eq!(field(&maps[0], "controls_mapped"), &Json::Int(11), "the skipped key is not tested, the rest are mapped");
        assert_eq!(field(&maps[0], "all_mapped"), &Json::Bool(false));
    }

    #[test]
    fn repeat_after_a_result_runs_the_step_again() {
        let mut hub = SimHub::sdk_pair();
        let mut o = opts();
        o.pause_pass_us = 1_000_000;
        let mut input = ScriptedInput::parse("js0-key-01 1 repeat\n").unwrap();
        // The scripted repeat fires on the first poll of attempt 1 (during the wait), so use a later poll: attempt 1 gets "repeat"
        // from the pause only if the step finished first; here it fires at once, which restarts the step; the second attempt passes.
        let (outcome, _, _) = run(&mut hub, &mut input, &o, &Selection::parse("keys").unwrap());
        assert_eq!(outcome.results[0].attempts.len(), 2);
        assert_eq!(outcome.results[0].outcome(), Outcome::Pass);
    }

    #[test]
    fn quit_records_the_rest_as_skipped() {
        let mut hub = SimHub::sdk_pair();
        let mut input = ScriptedInput::parse("js0-key-03 quit\n").unwrap();
        let (outcome, _, steps) = run(&mut hub, &mut input, &opts(), &Selection::all());
        assert!(outcome.stopped_early);
        assert_eq!(outcome.results.len(), steps.len());
        assert_eq!(outcome.results.iter().filter(|r| r.outcome() == Outcome::Pass).count(), 2);
        assert!(outcome.results[3..].iter().all(|r| r.outcome() == Outcome::Skipped));
        let back = parse(&result_json(&meta(), hub.devices(), &outcome).render()).unwrap();
        assert_eq!(field(&back, "stopped_early"), &Json::Bool(true));
        assert_eq!(field(&back, "complete"), &Json::Bool(false));
    }

    #[test]
    fn a_transposed_device_is_mapped_as_it_really_is() {
        // The consumers' current 3 x 4 table: keys answer as inputs 0-3, 8-11, 16-19 (bits 0-3 of D1..D3).
        let planner: Planner = Box::new(|step, _, view| {
            let st = &view.state[step.device];
            match &step.kind {
                StepKind::Press { slot, .. } => {
                    let (i, b) = (3 + slot / 4, (slot % 4) as u8);
                    vec![event(view.now_us + 500_000, step.device, vec![(i, st[i] ^ (1 << b))]), event(view.now_us + 800_000, step.device, vec![(i, st[i])])]
                }
                _ => Vec::new(),
            }
        });
        let mut hub = SimHub::new(vec![sim_desc(0x0426)], planner);
        let (outcome, screen, _) = run(&mut hub, &mut ScriptedInput::default(), &opts(), &Selection::parse("keys").unwrap());
        let back = parse(&result_json(&meta(), hub.devices(), &outcome).render()).unwrap();
        let Json::Arr(maps) = field(&back, "maps") else { panic!() };
        let m = &maps[0];
        assert_eq!(field(m, "all_mapped"), &Json::Bool(true));
        assert_eq!(field(m, "all_in_documented_set"), &Json::Bool(false), "bit 3 of D1 is the shuttle, not a key");
        let Json::Arr(rows) = field(m, "rows") else { panic!() };
        assert_eq!(field(&rows[3], "input"), &Json::Int(3));
        assert_eq!(field(&rows[3], "in_documented_set"), &Json::Bool(false));
        assert_eq!(field(&rows[3], "in_current_3x4_matrix"), &Json::Bool(true));
        assert!(screen.contains("NOT where the documentation puts it"));
    }

    #[test]
    fn pressing_the_wrong_control_twice_fails_the_second_step() {
        let planner: Planner = Box::new(|step, _, view| {
            let st = &view.state[step.device];
            match &step.kind {
                StepKind::Press { .. } => {
                    vec![event(view.now_us + 500_000, step.device, vec![(3, st[3] ^ 1)]), event(view.now_us + 800_000, step.device, vec![(3, st[3])])]
                }
                _ => Vec::new(),
            }
        });
        let mut hub = SimHub::new(vec![sim_desc(0x0426)], planner);
        let (outcome, _, _) = run(&mut hub, &mut ScriptedInput::parse("js0-key-01 skip").unwrap(), &opts(), &Selection::parse("keys").unwrap());
        assert_eq!(outcome.results[0].outcome(), Outcome::Skipped);
        assert_eq!(outcome.results[1].outcome(), Outcome::Pass);
        assert_eq!(outcome.results[2].outcome(), Outcome::Fail);
        assert!(outcome.results[2].attempts[0].note.contains("also what key 2 did"));
    }

    #[test]
    fn two_bits_at_once_are_reported_not_guessed() {
        let planner: Planner = Box::new(|step, _, view| {
            let st = &view.state[step.device];
            vec![event(view.now_us + 500_000, step.device, vec![(3, st[3] ^ 0x03)]), event(view.now_us + 800_000, step.device, vec![(3, st[3])])]
        });
        let mut hub = SimHub::new(vec![sim_desc(0x0426)], planner);
        let (outcome, _, _) = run(&mut hub, &mut ScriptedInput::default(), &opts(), &Selection::parse("keys").unwrap());
        assert_eq!(outcome.results[0].outcome(), Outcome::Fail);
        assert!(outcome.results[0].attempts[0].note.contains("2 bits changed"));
    }

    #[test]
    fn hotplug_fails_with_reasons_when_the_watch_stays_silent() {
        // The device goes away and comes back, but the sim's watch counter is not touched: replace Unplug/Replug by raw bytes.
        let planner: Planner = Box::new(|step, phase, view| {
            if phase == 0 {
                vec![SimEvent { at_us: view.now_us + 500_000, device: step.device, what: SimWhat::Unplug }]
            } else {
                Vec::new()
            }
        });
        let mut hub = SimHub::new(vec![sim_desc(0x04AB)], planner);
        let (outcome, screen, _) = run(&mut hub, &mut ScriptedInput::default(), &Options::unattended(3_000_000), &Selection::parse("hotplug").unwrap());
        assert_eq!(outcome.results[0].outcome(), Outcome::Fail);
        let note = &outcome.results[0].attempts[0].note;
        assert!(note.contains("could not be opened again"), "{note}");
        assert!(screen.contains("Stage 2 of 2: now PLUG"));
    }

    #[test]
    fn dry_run_skips_hotplug_and_a_missing_start_word_ends_cleanly() {
        let mut hub = SimHub::sdk_pair();
        let mut o = opts();
        o.skip_hotplug = true;
        let (outcome, screen, _) = run(&mut hub, &mut ScriptedInput::default(), &o, &Selection::parse("hotplug").unwrap());
        assert!(outcome.results.iter().all(|r| r.outcome() == Outcome::Skipped));
        assert!(screen.contains("skipped (needs a person)"));

        let mut hub = SimHub::sdk_pair();
        let mut o = Options::interactive();
        o.go_timeout_us = 40_000_000;
        let (outcome, screen, _) = run(&mut hub, &mut ScriptedInput::default(), &o, &Selection::all());
        assert!(outcome.not_started && outcome.results.is_empty());
        assert!(screen.contains("waiting for you to press Enter to begin (10 s left)"), "{screen}");
        assert!(screen.contains("Nobody pressed Enter within 40 s"));
    }

    #[test]
    fn jog_and_shuttle_steps_record_byte_values_and_flag_a_mismatch() {
        let mut hub = SimHub::sdk_pair();
        let (outcome, screen, _) = run(&mut hub, &mut ScriptedInput::default(), &opts(), &Selection::parse("jog").unwrap());
        let by_id = |id: &str| outcome.results.iter().find(|r| r.step.id == id).unwrap();
        assert!(by_id("js0-jog-cw").attempts[0].note.contains("8 detent(s) of step +1"), "{}", by_id("js0-jog-cw").attempts[0].note);
        assert!(by_id("js0-jog-ccw").attempts[0].note.contains("8 detent(s) of step -1"));
        assert!(by_id("js0-shuttle-cw").attempts[0].note.contains("reached 3 position(s) (+1, +2, +3)"));
        assert!(by_id("js0-shuttle-ccw").attempts[0].note.contains("reached 3 position(s) (-1, -2, -3)"));
        assert!(screen.contains("byte 8: 00 -> 01") && screen.contains("byte 8: 02 -> 03"), "{screen}");
    }

    // ---- recorded real sequences (a guided session on 2026-11-16 with an XK-12 Jog & Shuttle, 0426, and a Switch Interface, 04AB) -----

    /// A recorded report given as the leading hex bytes, zero-padded to the interface's input length.
    fn rec(hex: &str, len: usize) -> Vec<u8> {
        let mut v: Vec<u8> = hex.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect();
        v.resize(len, 0);
        v
    }

    /// One counter-clockwise detent as the device sent it, then the rest report 32 ms later.
    const JOG_NEG: &str = "00 02 00 00 80 80 00 FF 00 02 E4 16 6B";
    const JOG_POS: &str = "00 02 00 80 00 80 00 01 00 02 E4 2A 71";
    const JOG_REST: &str = "00 02 00 00 00 80 00 00 00 02 E4 16 8C";
    /// A report the guide (or another program) asked for with Generate Data: data type 2, resting state.
    const GENERATED_REST: &str = "00 02 02 00 00 80 00 00 00 02 E3 FA 0B";

    #[test]
    fn recorded_jog_reports_count_as_signed_detents_and_generated_reports_are_ignored() {
        let mut c = JogCounter::default();
        for _ in 0..3 {
            c.feed(&rec(JOG_NEG, 33));
            c.feed(&rec(GENERATED_REST, 33));
            c.feed(&rec(JOG_REST, 33));
        }
        for _ in 0..12 {
            c.feed(&rec(JOG_POS, 33));
            c.feed(&rec(JOG_REST, 33));
        }
        // A generated report that happens to carry a pulse (data type 2) is not a detent.
        c.feed(&rec("00 02 02 80 00 80 00 01", 33));
        assert_eq!((c.positive, c.negative, c.reports), (12, 3, 15));
        assert!(c.positive_flag && c.negative_flag);
        assert_eq!(c.values, BTreeSet::from([0x01, 0xFF]));
        assert_eq!(judge_jog(&c), JogVerdict::Turned { sign: 1, detents: 12 }, "the wrong-way start does not spoil a clear turn");

        let mut both = JogCounter::default();
        for _ in 0..8 {
            both.feed(&rec(JOG_POS, 33));
            both.feed(&rec(JOG_NEG, 33));
        }
        assert_eq!(judge_jog(&both), JogVerdict::Mixed);
        let mut few = JogCounter::default();
        few.feed(&rec(JOG_NEG, 33));
        few.feed(&rec(JOG_NEG, 33));
        assert_eq!(judge_jog(&few), JogVerdict::TooFew { sign: -1, detents: 2 });
        assert_eq!(judge_jog(&JogCounter::default()), JogVerdict::Nothing);
        // Faster turning gives larger signed counts; any signed value is handled.
        let mut fast = JogCounter::default();
        fast.feed(&rec("00 02 00 80 00 80 00 03", 33));
        fast.feed(&rec("00 02 00 00 80 80 00 FE", 33));
        assert_eq!((fast.positive, fast.negative), (3, 2));
    }

    /// Plays back a jog turn: `ccw` wrong-way detents then `cw` right-way ones, 150 ms apart with the rest report 32 ms later, and a
    /// Generate Data answer every 100 ms for `noise_s` seconds (as another program asking for the state would cause).
    fn jog_planner(wrong_way: usize, right_way: usize, noise_s: i64, negative_is_clockwise: bool) -> Planner {
        Box::new(move |step, _, view| {
            if !matches!(step.kind, StepKind::Jog { .. }) {
                return Vec::new();
            }
            let dev = step.device;
            let st = &view.state[dev];
            let pulse = |negative: bool| if negative { vec![(4usize, st[4] | 0x80), (7, 0xFF)] } else { vec![(3usize, st[3] | 0x80), (7, 0x01)] };
            let rest = || vec![(3usize, st[3]), (4, st[4]), (7, st[7])];
            let mut ev = Vec::new();
            let mut t = view.now_us + 600_000;
            let StepKind::Jog { clockwise } = step.kind else { unreachable!() };
            // The simulated person turns the way the step asks, but with a dial wired so that clockwise is `negative_is_clockwise`.
            let toward_negative = clockwise == negative_is_clockwise;
            for k in 0..(wrong_way + right_way) {
                let negative = if k < wrong_way { !toward_negative } else { toward_negative };
                ev.push(event(t, dev, pulse(negative)));
                ev.push(event(t + 32_000, dev, rest()));
                t += 150_000;
            }
            for k in 0..noise_s * 10 {
                ev.push(SimEvent { at_us: view.now_us + 100_000 + k * 100_000, device: dev, what: SimWhat::Generated });
            }
            ev
        })
    }

    fn jog_run(planner: Planner) -> (SessionOutcome, String) {
        let mut hub = SimHub::new(vec![sim_desc(0x0426)], planner);
        let (outcome, screen, _) = run(&mut hub, &mut ScriptedInput::default(), &Options::unattended(30_000_000), &Selection::parse("jog").unwrap());
        (outcome, screen)
    }

    #[test]
    fn a_jog_turn_passes_even_while_generate_data_answers_keep_arriving() {
        // The first real session failed here: another program's Generate Data answers, 100 ms apart, restarted the quiet timer.
        let (outcome, screen) = jog_run(jog_planner(3, 12, 25, false));
        let cw = &outcome.results[0];
        assert_eq!(cw.step.id, "js0-jog-cw");
        assert_eq!(cw.outcome(), Outcome::Pass, "{}", cw.attempts[0].note);
        assert!(cw.attempts[0].note.contains("12 detent(s) of step +1"), "{}", cw.attempts[0].note);
        assert!(cw.attempts[0].note.contains("direction flag byte 3 bit 7 seen"));
        assert!(!cw.attempts[0].note.contains("NOTE"), "positive is the expected clockwise direction");
        let generated = cw.attempts[0].reports.iter().filter(|r| r.generated).count();
        assert!(generated > 20, "the noise really was in the stream ({generated})");
        // The same dial turned the other way gives a negative count, which is what counter-clockwise should be.
        assert_eq!(outcome.results[1].outcome(), Outcome::Pass, "{}", outcome.results[1].attempts[0].note);
        assert!(outcome.results[1].attempts[0].note.contains("direction flag byte 4 bit 7 seen"));
        assert!(screen.contains("byte 4 bit 7 went ON") && screen.contains("byte 7: 00 -> FF"), "{screen}");
        let text = result_json(&meta(), &[sim_desc(0x0426)], &outcome).render();
        let back = parse(&text).unwrap();
        assert!(matches!(field(&back, "jog_note"), Json::Str(s) if s.contains("input number 7") && s.contains("input number 15")));
    }

    #[test]
    fn a_jog_wired_the_other_way_round_is_reported_as_a_mismatch_not_accepted_silently() {
        // Clockwise reporting negative: both steps get opposite signs, so both pass, but both carry the NOTE.
        let (outcome, _) = jog_run(jog_planner(0, 6, 0, true));
        assert_eq!(outcome.results[0].outcome(), Outcome::Pass);
        let note = &outcome.results[0].attempts[0].note;
        assert!(note.contains("NOTE: clockwise is expected to be positive, but clockwise reported negative"), "{note}");
        let back = parse(&result_json(&meta(), &[sim_desc(0x0426)], &outcome).render()).unwrap();
        let Json::Arr(steps) = field(&back, "steps") else { panic!() };
        let Json::Arr(a) = field(&steps[0], "attempts") else { panic!() };
        assert_eq!(field(field(&a[0], "response"), "matches_expectation"), &Json::Bool(false));
    }

    #[test]
    fn two_jog_steps_that_report_the_same_direction_fail_the_second() {
        // The person turns the same way both times: the planner ignores the requested direction.
        let planner: Planner = Box::new(|step, _, view| {
            let st = &view.state[step.device];
            if !matches!(step.kind, StepKind::Jog { .. }) {
                return Vec::new();
            }
            (0..6)
                .flat_map(|k| {
                    let t = view.now_us + 600_000 + k * 150_000;
                    [event(t, step.device, vec![(3, st[3] | 0x80), (7, 1)]), event(t + 32_000, step.device, vec![(3, st[3]), (7, 0)])]
                })
                .collect()
        });
        let (outcome, _) = jog_run(planner);
        assert_eq!(outcome.results[0].outcome(), Outcome::Pass);
        assert_eq!(outcome.results[1].outcome(), Outcome::Fail);
        assert!(outcome.results[1].attempts[0].note.contains("same direction (positive) as the clockwise step"), "{}", outcome.results[1].attempts[0].note);
    }

    #[test]
    fn a_jog_with_too_few_or_mixed_detents_fails_with_a_plain_message() {
        let (few, _) = jog_run(jog_planner(0, 2, 0, false));
        assert!(few.results[0].attempts[0].note.contains("only 2 detent(s) in one direction"));
        let (mixed, _) = jog_run(jog_planner(5, 6, 0, false));
        assert!(mixed.results[0].attempts[0].note.contains("turned both ways in similar amounts (6 detent(s) positive, 5 negative)"), "{}", mixed.results[0].attempts[0].note);
        let (none, _) = jog_run(Box::new(|_, _, _| Vec::new()));
        assert!(none.results[0].attempts[0].note.contains("nothing arrived from the Jog & Shuttle within 30 s"));
    }

    #[test]
    fn recorded_shuttle_positions_are_reported_as_what_the_device_sent() {
        // The real ring reported only +1 (D1 bit 3, byte 8 = 1, rest bit clear) and -1 (D1 bit 5, byte 8 = 0xFF).
        let plus_1 = rec("00 02 00 08 00 00 00 00 01 02 E5 A5 F9", 33);
        let minus_1 = rec("00 02 00 20 00 00 00 00 FF 02 E5 D3 F8", 33);
        let rest = rec("00 02 00 00 00 80 00 00 00 02 E5 D9 F7", 33);
        let mask = comparison_mask(Model::JogShuttle, 33, 9);
        let mut t = Tracker::new(rest.clone(), mask.clone());
        t.feed(&plus_1);
        t.feed(&rest);
        assert_eq!(shuttle_positions(&t.bytes[&8]), (BTreeSet::from([1]), BTreeSet::new()));
        let mut t = Tracker::new(rest.clone(), mask);
        t.feed(&minus_1);
        t.feed(&rest);
        assert_eq!(shuttle_positions(&t.bytes[&8]), (BTreeSet::new(), BTreeSet::from([1])));

        let planner = |bytes: Vec<(usize, u8)>| -> Planner {
            Box::new(move |step, _, view| {
                if !matches!(step.kind, StepKind::Shuttle { .. }) {
                    return Vec::new();
                }
                let st = &view.state[step.device];
                vec![event(view.now_us + 600_000, step.device, bytes.clone()), event(view.now_us + 2_000_000, step.device, vec![(3, st[3]), (5, st[5]), (8, st[8])])]
            })
        };
        let mut hub = SimHub::new(vec![sim_desc(0x0426)], planner(vec![(3, 0x08), (5, 0x00), (8, 0x01)]));
        let (outcome, _, _) = run(&mut hub, &mut ScriptedInput::default(), &opts(), &Selection::parse("jog").unwrap());
        let cw = outcome.results.iter().find(|r| r.step.id == "js0-shuttle-cw").unwrap();
        assert_eq!(cw.outcome(), Outcome::Pass);
        let note = &cw.attempts[0].note;
        assert!(note.contains("reached 1 position(s) (+1) and returned to rest"), "{note}");
        assert!(!note.contains("notches not seen"), "the old wording is gone");
        // The same +1 for the counter-clockwise step too: it must fail as "same side".
        let ccw = outcome.results.iter().find(|r| r.step.id == "js0-shuttle-ccw").unwrap();
        assert_eq!(ccw.outcome(), Outcome::Fail);
        assert!(ccw.attempts[0].note.contains("same side (positive) as the clockwise step"), "{}", ccw.attempts[0].note);
    }

    #[test]
    fn the_sim_shuttle_follows_the_documented_bits_and_both_sides_pass() {
        let mut hub = SimHub::sdk_pair();
        let (outcome, _, _) = run(&mut hub, &mut ScriptedInput::default(), &opts(), &Selection::parse("jog").unwrap());
        let n = |id: &str| outcome.results.iter().find(|r| r.step.id == id).unwrap().attempts[0].note.clone();
        assert!(n("js0-shuttle-cw").contains("reached 3 position(s) (+1, +2, +3)"), "{}", n("js0-shuttle-cw"));
        assert!(n("js0-shuttle-ccw").contains("reached 3 position(s) (-1, -2, -3)"), "{}", n("js0-shuttle-ccw"));
    }

    #[test]
    fn the_twelve_keys_map_as_measured_row_by_row() {
        // Measured: row by row from the top left, 4 keys per row, 3 rows: bit 0 top row, 1 middle, 2 bottom; bytes 3..6 along the row.
        let observed: Vec<(usize, u8)> = (0..12).map(|slot| bit_for_slot(Model::JogShuttle, slot).unwrap()).collect();
        assert_eq!(observed, [(3, 0), (4, 0), (5, 0), (6, 0), (3, 1), (4, 1), (5, 1), (6, 1), (3, 2), (4, 2), (5, 2), (6, 2)]);
        let inputs: Vec<u32> = observed.iter().map(|&(i, b)| input_number(i, b).unwrap()).collect();
        assert_eq!(inputs, [0, 8, 16, 24, 1, 9, 17, 25, 2, 10, 18, 26]);
        let rows = derive_map(&observed.iter().enumerate().map(|(k, &(i, b))| (format!("key {}", k + 1), Some(vec![BitChange { index: i, bit: b, set: true }]))).collect::<Vec<_>>());
        assert!(rows.iter().all(|r| r.status == MapStatus::Mapped));
        assert!(inputs.iter().all(|n| Model::JogShuttle.documented_inputs().contains(n)));
        // Four columns by three rows, not the consumers' Matrix(3, 4): 24..26 are real keys and 3, 11, 19 are not.
        assert!(inputs.iter().any(|n| !CURRENT_MATRIX_3X4.contains(n)));
    }

    // ---- Switch Interface sockets with one mono push button -----------------------------------------------------------------------

    #[test]
    fn plugging_in_changes_the_resting_state_which_is_what_the_old_flow_tripped_over() {
        // From the recording: with a plug in socket 1 the rest state was byte 3 = 01 (empty is 00), and pressing took it to 00.
        let mask = comparison_mask(Model::SwitchInterface12, 37, 32);
        let empty = rec("00 01 02 00", 37);
        let plugged = rec("00 01 02 01", 37);
        assert_eq!(diff_bits(&mask, &empty, &plugged), vec![BitChange { index: 3, bit: 0, set: true }]);
        assert_eq!(diff_bits(&mask, &plugged, &empty), vec![BitChange { index: 3, bit: 0, set: false }], "pressed: ON at rest, OFF when pressed");
    }

    #[test]
    fn socket_overlaps_find_shared_bits() {
        let b = |index, bit| BitChange { index, bit, set: true };
        let r = |socket, plug: Vec<BitChange>, button: Vec<BitChange>| SocketResult { socket, plug, button, released_to_rest: true, unplugged_to_empty: true, unique: true };
        let rows = [r(0, vec![b(3, 1)], vec![b(3, 0)]), r(1, vec![b(3, 1)], vec![b(3, 2)]), r(2, vec![b(3, 5)], vec![b(3, 4)])];
        assert_eq!(socket_overlaps(&rows), vec![vec![2], vec![1], vec![]]);
    }

    #[test]
    fn socket_steps_find_the_plug_bits_and_the_button_bit_with_its_polarity() {
        let mut hub = SimHub::sdk_pair();
        let (outcome, screen, _) = run(&mut hub, &mut ScriptedInput::default(), &opts(), &Selection::parse("inputs").unwrap());
        assert_eq!(outcome.results.len(), 6);
        for r in &outcome.results {
            assert_eq!(r.outcome(), Outcome::Pass, "{}: {}", r.step.id, r.attempts[0].note);
        }
        let first = outcome.results[0].attempts[0].socket.clone().unwrap();
        // Socket 1, as measured: plugging in sets byte 3 bit 0; pressing the button sets bit 1 (it rests OFF).
        assert_eq!(first.plug, vec![BitChange { index: 3, bit: 0, set: true }]);
        assert_eq!(first.button, vec![BitChange { index: 3, bit: 1, set: true }]);
        assert!(first.released_to_rest && first.unplugged_to_empty && first.unique);
        assert!(outcome.results[0].attempts[0].note.contains("the button is byte 3 bit 1 (input number 1), which goes ON when pressed (rests OFF while plugged in)"), "{}", outcome.results[0].attempts[0].note);
        for s in ["Socket 1 of 6", "First make sure NOTHING is plugged", ">> Plug the push button into socket 1 now.", ">> Now press the button and HOLD it down.", ">> Now LET GO of the button (leave it plugged in).", ">> Now UNPLUG the button from socket 1.", "ONE push button wired to a mono (tip and sleeve) jack plug", "type  right"] {
            assert!(screen.contains(s), "missing \"{s}\" in\n{screen}");
        }
        assert_eq!(screen.matches("ONE push button wired").count(), 1, "the intro is said once");
        let back = parse(&result_json(&meta(), hub.devices(), &outcome).render()).unwrap();
        let Json::Arr(maps) = field(&back, "socket_maps") else { panic!() };
        assert_eq!(maps.len(), 1);
        assert_eq!(field(&maps[0], "all_mapped"), &Json::Bool(true));
        let Json::Arr(rows) = field(&maps[0], "rows") else { panic!() };
        assert_eq!(rows.len(), 6);
        assert_eq!(field(field(&rows[1], "button_bit"), "bit"), &Json::Int(3));
        assert_eq!(field(field(&rows[1], "button_bit"), "resting_level_when_plugged_in"), &Json::Str("OFF".into()));
        assert_eq!(field(&rows[0], "shares_bits_with_sockets"), &Json::Arr(vec![]));
        assert_eq!(field(&back, "switch_interface_sockets_counted_from"), &Json::Str("left (assumed)".into()));
    }

    #[test]
    fn a_socket_that_reuses_another_sockets_button_bit_fails_and_is_flagged() {
        // Every socket behaves like socket 1.
        let planner: Planner = Box::new(|step, phase, view| {
            if !matches!(step.kind, StepKind::Socket { .. }) {
                return Vec::new();
            }
            let (plugged, pressed) = match phase {
                0 => (true, false),
                1 => (true, true),
                2 => (true, false),
                _ => (false, false),
            };
            vec![event(view.now_us + 600_000, step.device, socket_bytes(&view.state[step.device], 0, plugged, pressed))]
        });
        let mut hub = SimHub::new(vec![sim_desc(0x04AB)], planner);
        let (outcome, _, _) = run(&mut hub, &mut ScriptedInput::default(), &opts(), &Selection::parse("inputs").unwrap());
        assert_eq!(outcome.results[0].outcome(), Outcome::Pass);
        assert_eq!(outcome.results[1].outcome(), Outcome::Fail);
        assert!(outcome.results[1].attempts[0].note.contains("same one socket 1's button used"), "{}", outcome.results[1].attempts[0].note);
        let back = parse(&result_json(&meta(), hub.devices(), &outcome).render()).unwrap();
        let Json::Arr(maps) = field(&back, "socket_maps") else { panic!() };
        let Json::Arr(rows) = field(&maps[0], "rows") else { panic!() };
        assert_eq!(field(&rows[1], "status"), &Json::Str("duplicate".into()));
        assert_eq!(field(&rows[0], "shares_bits_with_sockets"), &Json::Arr(vec![2, 3, 4, 5, 6].into_iter().map(Json::Int).collect()));
    }

    #[test]
    fn nothing_plugged_in_times_out_with_a_plain_message() {
        let mut hub = SimHub::new(vec![sim_desc(0x04AB)], Box::new(|_, _, _| Vec::new()));
        let (outcome, screen, _) = run(&mut hub, &mut ScriptedInput::default(), &Options::unattended(3_000_000), &Selection::parse("inputs").unwrap());
        assert!(outcome.results.iter().all(|r| r.outcome() == Outcome::Fail));
        assert!(outcome.results[0].attempts[0].note.contains("nothing changed within 3 s while waiting for the button to be plugged in"));
        assert!(screen.contains("...still waiting") || screen.contains("Waiting up to 3 s"));
    }

    #[test]
    fn which_end_the_sockets_were_counted_from_is_recorded() {
        let mut hub = SimHub::sdk_pair();
        let mut input = ScriptedInput::parse("si1-socket-1 right\n").unwrap();
        let (outcome, screen, _) = run(&mut hub, &mut input, &opts(), &Selection::parse("inputs").unwrap());
        assert_eq!(outcome.counting_from, "right");
        assert!(screen.contains("Noted: you count the Switch Interface's sockets from the right."));
        assert_eq!(outcome.results[0].outcome(), Outcome::Pass, "the answer does not disturb the step");
        let back = parse(&result_json(&meta(), hub.devices(), &outcome).render()).unwrap();
        assert_eq!(field(&back, "switch_interface_sockets_counted_from"), &Json::Str("right".into()));
        assert_eq!(parse_command("Right"), Some(Command::Right));
        assert_eq!(parse_command("left"), Some(Command::Left));
    }

    #[test]
    fn the_per_contact_flow_is_still_available_and_timing_uses_the_button_by_default() {
        let all = plan_steps(&pair_devices(), &Selection::parse("all,per-contact").unwrap(), 5);
        assert_eq!(all.len(), 12 + 4 + 12 + 3 + 2 + 16, "the previous 33 steps plus the light steps");
        assert!(all.iter().any(|s| s.id == "si0-input-1R") && !all.iter().any(|s| s.id.contains("socket")));
        let default = plan_steps(&pair_devices(), &Selection::all(), 5);
        assert_eq!(default.len(), 12 + 4 + 6 + 2 + 2 + 16);
        assert!(default.iter().all(|s| !s.id.contains("input-")), "no per-contact steps by default");
        assert_eq!(default.iter().filter(|s| s.group == Group::Timing).map(|s| s.id.as_str()).collect::<Vec<_>>(), ["js1-timing", "cross-timing"], "no two-inputs step for one button");
        let cross = default.iter().find(|s| s.id == "cross-timing").unwrap();
        assert_eq!(cross.kind, StepKind::Pair { reps: 5, slots: (0, 0), plug_socket: Some(0) });
        assert!(cross.prompt.contains("press the button and any key on the Jog & Shuttle"), "{}", cross.prompt);
        let first = default.iter().find(|s| s.id == "si0-socket-1").unwrap();
        assert!(first.prompt.starts_with("Socket 1 of 6"));
    }

    #[test]
    fn the_cross_device_step_asks_for_the_button_to_be_plugged_in_first() {
        let mut hub = SimHub::sdk_pair();
        let (outcome, screen, _) = run(&mut hub, &mut ScriptedInput::default(), &opts(), &Selection::parse("timing").unwrap());
        assert_eq!(outcome.results.len(), 2);
        for r in &outcome.results {
            assert_eq!(r.outcome(), Outcome::Pass, "{}: {}", r.step.id, r.attempts[0].note);
        }
        assert!(screen.contains(">> First plug the push button into socket 1 of the Switch Interface, and leave it plugged in."), "{screen}");
        let rounds = outcome.results[1].attempts[0].response.clone();
        assert!(matches!(rounds, Json::Obj(_)));
    }

    // ---- shuttle overshoot and timing note ---------------------------------------------------------------------------------------

    #[test]
    fn a_single_minus_one_at_the_end_of_a_clockwise_sweep_is_the_spring_not_a_second_direction() {
        // Recorded in a guided session (js0-shuttle-cw): +1 ... +7, back to +6, then -1, then rest.
        let recorded: [i8; 11] = [1, 2, 3, 4, 5, 6, 7, 6, -1, 0, 0];
        let v = shuttle_sweep(&recorded);
        assert_eq!(v.sign, Some(1));
        assert_eq!(v.positions, BTreeSet::from([1, 2, 3, 4, 5, 6, 7]));
        assert_eq!(v.overshoot, vec![-1]);
        assert!(!v.both_sides);
        // The mirror image.
        let ccw = shuttle_sweep(&[-1, -2, -3, 0, 1, 0]);
        assert_eq!((ccw.sign, ccw.overshoot.clone(), ccw.both_sides), (Some(-1), vec![1], false));
        // A real move to the other side, an overshoot in the middle, or a bigger overshoot is not tolerated.
        assert!(shuttle_sweep(&[1, 2, -1, 3, 0]).both_sides);
        assert!(shuttle_sweep(&[1, 2, 3, -2, 0]).both_sides);
        assert!(shuttle_sweep(&[1, 2, 3, -1, -1, -1, 0]).both_sides, "three reports is a move, not a bounce");
        assert!(shuttle_sweep(&[1, 3, -1, -3]).both_sides);
        assert_eq!(shuttle_sweep(&[0, 0]).sign, None);
        assert!(shuttle_sweep(&[1, 0]).overshoot.is_empty());
    }

    #[test]
    fn the_shuttle_step_passes_with_the_recorded_overshoot_and_says_so() {
        // (bit value, byte 8 position, byte index of the bit): +1..+4 are bit 3 of D1..D4, +5..+7 bit 4 of D1..D3, -1 bit 5 of D1.
        let seq: Vec<(u8, u8, usize)> = vec![(0x08, 1, 3), (0x08, 2, 4), (0x08, 3, 5), (0x08, 4, 6), (0x10, 5, 3), (0x10, 6, 4), (0x10, 7, 5), (0x10, 6, 4), (0x20, 0xFF, 3)];
        let planner: Planner = Box::new(move |step, _, view| {
            if !matches!(step.kind, StepKind::Shuttle { clockwise: true }) {
                return Vec::new();
            }
            let st = &view.state[step.device];
            let mut ev = Vec::new();
            for (k, (bit, pos, byte)) in seq.iter().enumerate() {
                let mut bytes: Vec<(usize, u8)> = vec![(3, 0), (4, 0), (5, 0), (6, 0), (8, *pos)];
                bytes[*byte - 3].1 = *bit;
                ev.push(event(view.now_us + 600_000 + k as i64 * 300_000, step.device, bytes));
            }
            ev.push(event(view.now_us + 600_000 + 10 * 300_000, step.device, vec![(3, st[3]), (4, st[4]), (5, st[5]), (6, st[6]), (8, st[8])]));
            ev
        });
        let mut hub = SimHub::new(vec![sim_desc(0x0426)], planner);
        let (outcome, _, _) = run(&mut hub, &mut ScriptedInput::default(), &opts(), &Selection::parse("jog").unwrap());
        let cw = outcome.results.iter().find(|r| r.step.id == "js0-shuttle-cw").unwrap();
        assert_eq!(cw.outcome(), Outcome::Pass, "{}", cw.attempts[0].note);
        let note = &cw.attempts[0].note;
        assert!(note.contains("reached 7 position(s) (+1, +2, +3, +4, +5, +6, +7)"), "{note}");
        assert!(note.contains("a brief overshoot to the other side (-1) was seen as the ring sprang back to rest, and ignored"), "{note}");
    }

    #[test]
    fn the_timing_note_says_the_smallest_gap_between_consecutive_reports() {
        let mut hub = SimHub::sdk_pair();
        let (outcome, _, _) = run(&mut hub, &mut ScriptedInput::default(), &opts(), &Selection::parse("timing").unwrap());
        let js = outcome.results.iter().find(|r| r.step.id == "js0-timing").unwrap();
        // The simulated rounds use gaps of 0, 2, 0, 7 and 0.5 ms; the smallest non-zero one is 0.5 ms.
        assert!(js.attempts[0].note.contains("smallest gap between consecutive reports from the Jog & Shuttle: 0.5 ms"), "{}", js.attempts[0].note);
        let back = parse(&result_json(&meta(), hub.devices(), &outcome).render()).unwrap();
        let Json::Arr(steps) = field(&back, "steps") else { panic!() };
        let Json::Arr(a) = field(&steps[0], "attempts") else { panic!() };
        let Json::Arr(g) = field(field(&a[0], "response"), "smallest_gap_between_reports") else { panic!() };
        assert_eq!(field(&g[0], "gap_us"), &Json::Int(500));
    }

    // ---- lights ---------------------------------------------------------------------------------------------------------------

    fn light_run(hub: &mut SimHub, input: &mut dyn Input, only: &str) -> (SessionOutcome, String) {
        let (outcome, screen, _) = run(hub, input, &opts(), &Selection::parse(only).unwrap());
        (outcome, screen)
    }

    #[test]
    fn answers_are_parsed_and_judged() {
        assert_eq!(parse_answer(Ask::YesNo, " Y "), Ok(Answer::Yes));
        assert_eq!(parse_answer(Ask::YesNo, "no"), Ok(Answer::No));
        assert!(parse_answer(Ask::YesNo, "maybe").is_err());
        assert_eq!(parse_answer(Ask::Keys, "1 2, 3 4"), Ok(Answer::Keys(BTreeSet::from([1, 2, 3, 4]))));
        assert_eq!(parse_answer(Ask::Keys, "none"), Ok(Answer::Keys(BTreeSet::new())));
        assert!(parse_answer(Ask::Keys, "13").is_err() && parse_answer(Ask::Keys, "a").is_err() && parse_answer(Ask::Keys, "").is_err());
        let want = Expect::KeysLit(BTreeSet::from([1, 2, 3, 4]));
        assert_eq!(judge_light_answer(Ask::Keys, &want, &Answer::Keys(BTreeSet::from([1, 2, 3, 4]))).0, "pass");
        let (v, d) = judge_light_answer(Ask::Keys, &want, &Answer::Keys(BTreeSet::from([5, 6, 7, 8])));
        assert_eq!(v, "fail");
        assert!(d.contains("keys lit: 5 6 7 8") && d.contains("belong to keys 1 2 3 4"));
        assert_eq!(judge_light_answer(Ask::Observe, &Expect::Rows { bank: 0, mask: 1 }, &Answer::Keys(BTreeSet::from([1, 5, 9]))).0, "recorded");
        assert_eq!(judge_light_answer(Ask::YesNo, &Expect::Nothing, &Answer::No).0, "fail");
    }

    #[test]
    fn scripted_input_carries_answers_for_light_questions() {
        let mut s = ScriptedInput::parse("a y\na n\nb 1 2 3 4\nc #2 none\nd skip\n").unwrap();
        assert_eq!((s.answer("a", 1), s.answer("a", 1), s.answer("a", 1)), (Some("y".into()), Some("n".into()), None));
        assert_eq!(s.answer("b", 1), Some("1 2 3 4".into()));
        assert_eq!(s.answer("c", 1), None);
        assert_eq!(s.answer("c", 2), Some("none".into()));
        assert_eq!(s.poll("d", 1), Some(Command::Skip));
    }

    #[test]
    fn the_light_plans_use_only_documented_volatile_commands_and_exercise_every_backlight() {
        let js = jog_shuttle_light_plan();
        let si = switch_interface_light_plan("XK-12 Switch Interface");
        let sent = |plan: &[LightStepPlan]| -> Vec<Vec<u8>> { plan.iter().flat_map(|(_, _, _, ph)| ph.iter().flat_map(|p| p.sends.iter().map(|c| c.bytes.clone()))).collect() };
        for r in sent(&js) {
            assert_eq!(lights::validate_light_report(LightModel::JogShuttle, &r), Ok(()), "{r:?}");
            assert!(!lights::NEVER_SENT.iter().any(|(c, _)| *c == r[1]));
        }
        for r in sent(&si) {
            assert_eq!(lights::validate_light_report(LightModel::SwitchInterface, &r), Ok(()), "{r:?}");
            assert_ne!(r[1], lights::CMD_SET_LEDS, "the Switch Interface has no set-both command");
        }
        // Every key's backlight, in both banks, is switched on individually by command 181 in a question that asks which keys are lit.
        let mut lit: BTreeSet<(u8, u8)> = BTreeSet::new();
        for (_, _, _, phases) in &js {
            for p in phases.iter().filter(|p| p.ask == Ask::Keys) {
                for c in &p.sends {
                    if c.bytes[1] == lights::CMD_SET_BACKLIGHT && c.bytes[3] == 1 {
                        lit.insert((c.bytes[2] / 32, c.bytes[2] % 32));
                    }
                }
            }
        }
        let all: BTreeSet<(u8, u8)> = (0..2u8).flat_map(|b| lights::JOG_SHUTTLE_KEY_INDEXES.iter().map(move |&k| (b, k))).collect();
        assert_eq!(lit, all);
        // The expected keys of each row question are the numbers of the keys whose indexes were switched on.
        let rows: Vec<&LightPhase> = js.iter().filter(|(id, ..)| *id == "bank1-keys").flat_map(|(_, _, _, ph)| ph.iter()).collect();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].expect, Expect::KeysLit(BTreeSet::from([5, 6, 7, 8])));
        assert!(rows[1].sends.iter().any(|c| c.bytes[1] == 181 && c.bytes[2] == 1), "row 2 starts with the key whose index is 1");
        // The Switch Interface's outputs are gated by a question.
        let out1 = si.iter().find(|(id, ..)| *id == "out1").unwrap();
        assert_eq!(out1.3[0].ask, Ask::Gate);
        assert!(out1.3[0].sends.is_empty());
    }

    #[test]
    fn cleanup_switches_everything_off_in_documented_commands_only() {
        let js = cleanup_commands(Model::JogShuttle, &[], false);
        for c in &js {
            assert_eq!(lights::validate_light_report(LightModel::JogShuttle, &c.bytes), Ok(()), "{}", c.label);
        }
        assert_eq!(js[0].bytes[..4], [0, 179, 6, 0]);
        assert_eq!(js[1].bytes[..4], [0, 179, 7, 0]);
        let offs: BTreeSet<u8> = js.iter().filter(|c| c.bytes[1] == 181 && c.bytes[3] == 0).map(|c| c.bytes[2]).collect();
        assert_eq!(offs.len(), 24);
        assert!(offs.contains(&0) && offs.contains(&58), "bank 1 key 0 and bank 2 key 26 (index 58)");
        assert_eq!(js.last().unwrap().bytes[..4], [0, 187, 255, 255]);
        assert!(!js.iter().any(|c| c.bytes[1] == 184));
        let odd = cleanup_commands(Model::JogShuttle, &[], true);
        assert_eq!(odd[2].bytes[1], 184, "an odd number of toggles is undone first");
        let si = cleanup_commands(Model::SwitchInterface12, &[], false);
        assert_eq!(si.len(), 2);
        let si_out = cleanup_commands(Model::SwitchInterface12, &[0, 1], false);
        assert_eq!(si_out.iter().map(|c| c.bytes[2]).collect::<Vec<_>>(), [6, 7, 0, 1]);
    }

    #[test]
    fn a_perfect_person_passes_the_lights_part_and_everything_ends_off() {
        let mut hub = SimHub::sdk_pair();
        let (outcome, screen) = light_run(&mut hub, &mut ScriptedInput::default(), "lights");
        for r in &outcome.results {
            assert_eq!(r.outcome(), Outcome::Pass, "{}: {}", r.step.id, r.attempts[0].note);
        }
        // 11 Jog & Shuttle steps (10 lights + cleanup) and 5 Switch Interface steps.
        assert_eq!(outcome.results.len(), 16);
        assert!(screen.contains("The lights part: I will now switch the lights"));
        assert_eq!(screen.matches("The lights part: I will now").count(), 1);
        assert!(screen.contains(">> (1 of 4) The GREEN indicator light (the small light at the top left of the case, above the keys) should now be ON and steady. Is it? (y/n)"), "{screen}");
        assert!(screen.contains("Which keys are lit? Type their numbers separated by spaces, for example 1 2 3 4"));
        assert!(screen.contains("every light on the Jog & Shuttle that this tool switched is back OFF"));
        // Nothing but documented volatile light commands reached a device.
        let commands: BTreeSet<u8> = hub.writes.iter().map(|(_, r)| r[1]).collect();
        assert_eq!(commands, BTreeSet::from([179, 180, 181, 182, 184, 186, 187]));
        assert!(hub.writes.iter().all(|(_, r)| r.len() == 36 && r[0] == 0));
        assert!(commands.iter().all(|c| !lights::NEVER_SENT.iter().any(|(n, _)| n == c)));
        // The Switch Interface got only 179 and 180.
        assert!(hub.writes.iter().filter(|(d, _)| *d == 1).all(|(_, r)| matches!(r[1], 179 | 180)));
        // The outputs were exercised (the simulated person had something connected) and are off again, like every other light.
        for d in 0..2 {
            let l = &hub.lights[d];
            assert!(l.led.iter().all(|&v| v == 0), "device {d}: {:?}", l.led);
            assert!(l.backlight.iter().all(|&v| v == 0));
            assert_eq!(l.intensity, [255, 255]);
            assert!(!l.toggled_off, "the toggle was undone");
        }
        assert!(hub.writes.iter().any(|(d, r)| *d == 1 && r[1] == 179 && r[2] == 0 && r[3] == 1), "OUT 1 was switched on");
        // The result file records the exact bytes, the write result and the answer for each question.
        let back = parse(&result_json(&meta(), hub.devices(), &outcome).render()).unwrap();
        let Json::Arr(steps) = field(&back, "steps") else { panic!() };
        let green = steps.iter().find(|s| field(s, "id") == &Json::Str("js0-light-green".into())).unwrap();
        let Json::Arr(attempts) = field(green, "attempts") else { panic!() };
        let Json::Arr(questions) = field(field(&attempts[0], "response"), "questions") else { panic!() };
        assert_eq!(questions.len(), 4);
        let Json::Arr(sent) = field(&questions[0], "sent") else { panic!() };
        assert_eq!(field(&sent[0], "bytes"), &Json::Str(format!("00 B3 06 01{}", " 00".repeat(32))));
        assert_eq!(field(&sent[0], "write"), &Json::Str("ok".into()));
        assert_eq!(field(&questions[0], "answer"), &Json::Str("y".into()));
        assert_eq!(field(&questions[0], "verdict"), &Json::Str("pass".into()));
        let Json::Arr(table) = field(&back, "light_commands") else { panic!() };
        let row = |c: i64| table.iter().find(|r| field(r, "command") == &Json::Int(c)).unwrap();
        assert_eq!(field(row(189), "volatile"), &Json::Bool(false));
        assert_eq!(field(row(189), "sent_by_the_lights_part"), &Json::Bool(false));
        assert_eq!(field(row(181), "volatile"), &Json::Bool(true));
        assert_eq!(field(row(183), "volatile"), &Json::Null);
    }

    #[test]
    fn a_wrong_backlight_index_shows_up_in_the_key_questions() {
        let mut hub = SimHub::sdk_pair();
        hub.backlight_index_quirk = Some(|i| i + 1);
        let (outcome, _) = light_run(&mut hub, &mut ScriptedInput::default(), "lights");
        let bank1 = outcome.results.iter().find(|r| r.step.id == "js0-light-bank1-keys").unwrap();
        assert_eq!(bank1.outcome(), Outcome::Fail);
        assert!(bank1.attempts[0].note.contains("keys lit: 5 6 7 8; the backlights switched on belong to keys 1 2 3 4"), "{}", bank1.attempts[0].note);
    }

    #[test]
    fn a_no_answer_fails_and_a_declined_output_is_skipped_without_touching_it() {
        let mut hub = SimHub::sdk_pair();
        let mut input = ScriptedInput::parse("si1-light-out1 n\njs0-light-red n\njs0-light-red n\n").unwrap();
        let (outcome, _) = light_run(&mut hub, &mut input, "lights");
        let out1 = outcome.results.iter().find(|r| r.step.id == "si1-light-out1").unwrap();
        assert_eq!(out1.outcome(), Outcome::Skipped);
        assert!(out1.attempts[0].note.contains("declined"));
        assert!(!hub.writes.iter().any(|(d, r)| *d == 1 && r[1] == 179 && r[2] == 0), "OUT 1 was never touched, not even by the cleanup");
        assert!(hub.writes.iter().any(|(d, r)| *d == 1 && r[1] == 179 && r[2] == 1 && r[3] == 0), "OUT 2 was exercised and switched off again");
        let red = outcome.results.iter().find(|r| r.step.id == "js0-light-red").unwrap();
        assert_eq!(red.outcome(), Outcome::Fail, "an n answer is a failure");
        assert!(red.attempts[0].note.contains("question 1: the person answered no"));
        // With nobody answering, the step times out saying which question it waited for.
        let mut quiet = SimHub::sdk_pair();
        quiet.person_answers = false;
        let (o, _) = light_run(&mut quiet, &mut ScriptedInput::default(), "lights");
        assert!(o.results[0].attempts[0].note.contains("no answer within 5 s to question 1: The GREEN indicator light"), "{}", o.results[0].attempts[0].note);
        assert_eq!(o.results.last().unwrap().outcome(), Outcome::Pass, "the cleanup still ran");
        assert!(quiet.lights[0].led.iter().all(|&v| v == 0));
    }

    #[test]
    fn quitting_part_way_still_puts_the_lights_back() {
        let mut hub = SimHub::sdk_pair();
        let mut input = ScriptedInput::parse("js0-light-bank1-keys quit\n").unwrap();
        let (outcome, _) = light_run(&mut hub, &mut input, "lights");
        assert!(outcome.stopped_early);
        let cleanup = outcome.results.iter().find(|r| r.step.id == "js0-lights-off").unwrap();
        assert_eq!(cleanup.outcome(), Outcome::Pass, "{}", cleanup.attempts[0].note);
        assert!(hub.lights[0].backlight.iter().all(|&v| v == 0) && hub.lights[0].led.iter().all(|&v| v == 0));
        let later = outcome.results.iter().find(|r| r.step.id == "js0-light-toggle").unwrap();
        assert_eq!(later.outcome(), Outcome::Skipped);
        let si_cleanup = outcome.results.iter().find(|r| r.step.id == "si1-lights-off").unwrap();
        assert_eq!(si_cleanup.outcome(), Outcome::Skipped, "a device whose lights were never touched is left alone");
    }

    #[test]
    fn a_hub_refuses_a_report_that_is_not_a_light_command() {
        let mut hub = SimHub::sdk_pair();
        let mut set_unit_id = [0u8; 36];
        set_unit_id[1] = 189;
        set_unit_id[2] = 7;
        assert!(hub.send_light(0, &set_unit_id).unwrap_err().contains("never sent"));
        let mut save = [0u8; 36];
        save[1] = 199;
        save[2] = 1;
        assert!(hub.send_light(0, &save).is_err());
        assert!(hub.writes.is_empty());
    }
}
