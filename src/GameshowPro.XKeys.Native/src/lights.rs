// Copyright (c) Barjonas LLC 2026.
//! Output reports that control the lights of the XK-12 Jog & Shuttle and the XK-12 Switch Interface, as pure functions (no I/O).
//! Each builder returns the full 36-byte output report: byte 0 is the report ID (0), byte 1 the command, then its parameters, then
//! zeros. The managed library mirrors these builders; the unit tests pin the exact bytes.
//!
//! Sources (P.I. Engineering SDK documentation, <https://github.com/piengineering/PI-Engineering-SDK>, folder
//! `Documentation/pages`):
//! - `X-keys XK-12 Jog & Shuttle Data Report.htm` and `X-keys XK-12 Jog Shuttle Data Report.htm` ("Output Report" section: report
//!   list, items 1 to 14; Figure 1 `images/XK-12JogShuttle.jpg` shows the key numbers 0,8,16,24 / 1,9,17,25 / 2,10,18,26 by row).
//! - `X-keys XK-12 XK-3 Switch Interface Data Report.htm` ("Output Report" section, items 1 to 3).
//!
//! The pages mark with a superscript "e" the commands that write to the device's EEPROM (limit: 50,000 writes): Set Unit ID (189),
//! Save Backlight State (199) and Change PID (204) on the Jog & Shuttle; Set Unit ID, Change PID, Set Version Number (195), Set
//! Dongle Key (192) on the Switch Interface. **None of those is built here, and [`validate_light_report`] refuses them.** Every
//! command built here is unmarked in the documentation, that is, volatile: it changes the device's present state only.
//!
//! Index and unit conventions are the documentation's, with one flagged inconsistency: the Jog & Shuttle page's Index Based Set
//! Backlights example says "lower left key bank 1 index=5", but Figure 1 (and the newer page's text, "value shown on key in
//! Figure 1") number that key 2. This module follows the figure; the guided test (`xkprobe guide --only lights`) asks the person
//! which keys light, so a wrong index shows.

/// Length of an output report on these devices, report ID included.
pub const OUTPUT_REPORT_LEN: usize = 36;

/// 186, Set LEDs (Jog & Shuttle only): one byte, bit 6 (0x40) = green, bit 7 (0x80) = red. Volatile.
pub const CMD_SET_LEDS: u8 = 186;
/// 179, Index Based Set LED (Flash): LED index, state. Volatile. Both models.
pub const CMD_SET_LED: u8 = 179;
/// 180, Set Frequency of Flash: 1 (fastest) to 255 (slowest, about 4 s between flashes). Volatile. Both models.
pub const CMD_SET_FLASH_FREQUENCY: u8 = 180;
/// 181, Index Based Set Backlights (Flash) (Jog & Shuttle only): key index (+32 for bank 2), state. Volatile.
pub const CMD_SET_BACKLIGHT: u8 = 181;
/// 182, Turn On/Off Rows of Backlights (Jog & Shuttle only): bank, bit mask. Volatile.
pub const CMD_SET_BACKLIGHT_ROWS: u8 = 182;
/// 184, Toggle Backlights (Jog & Shuttle only). Volatile.
pub const CMD_TOGGLE_BACKLIGHTS: u8 = 184;
/// 187, Set Backlight Intensity (Jog & Shuttle only): bank 1 and bank 2 intensity, 0 to 255. Volatile.
pub const CMD_SET_BACKLIGHT_INTENSITY: u8 = 187;

/// Commands that are never sent by this tool, and why (the documentation marks the first three with "e"; the others change mode,
/// identity or behavior, or are not light commands).
pub const NEVER_SENT: [(u8, &str); 10] = [
    (189, "Set Unit ID: writes EEPROM"),
    (199, "Save Backlight State to EEPROM: writes EEPROM"),
    (204, "Change PID: writes EEPROM and changes identity"),
    (238, "Reboot Device"),
    (210, "Enable Time Stamp: changes the report format"),
    (183, "Enable Scroll Lock to Toggle Backlights: changes a keyboard-driven mode; persistence not stated"),
    (195, "Set Version Number: writes EEPROM"),
    (192, "Set Dongle Key: writes EEPROM"),
    (214, "Request Descriptor: not a light command"),
    (177, "Generate Data: not a light command (sent elsewhere by the guide to read the state)"),
];

/// LED indexes of command 179.
pub const LED_OUT1: u8 = 0;
pub const LED_OUT2: u8 = 1;
pub const LED_GREEN: u8 = 6;
pub const LED_RED: u8 = 7;

/// The bits of command 186.
pub const LEDS_GREEN: u8 = 0x40;
pub const LEDS_RED: u8 = 0x80;

/// State of a light for commands 179 and 181.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightState {
    Off = 0,
    On = 1,
    /// Flashes at the frequency set with command 180.
    Flash = 2,
}

/// Keys of the Jog & Shuttle: documentation key numbers 0,8,16,24 / 1,9,17,25 / 2,10,18,26 (Figure 1), which is also the key's
/// backlight index for bank 1. Position `n` (1 to 12) counts along each row from the left, top row first (the guide's key steps).
pub const JOG_SHUTTLE_KEY_INDEXES: [u8; 12] = [0, 8, 16, 24, 1, 9, 17, 25, 2, 10, 18, 26];

/// Backlight banks of the Jog & Shuttle: bank 0 and bank 1 here are the documentation's bank 1 and bank 2.
pub const BANKS: u8 = 2;

/// Why a report could not be built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightError(pub String);

fn report(command: u8, params: &[u8]) -> [u8; OUTPUT_REPORT_LEN] {
    let mut r = [0u8; OUTPUT_REPORT_LEN];
    r[1] = command;
    r[2..2 + params.len()].copy_from_slice(params);
    r
}

/// 186: set the green and red indicator LEDs together (Jog & Shuttle). `[0, 186, leds, 0...]`.
pub fn set_leds(green: bool, red: bool) -> [u8; OUTPUT_REPORT_LEN] {
    report(CMD_SET_LEDS, &[if green { LEDS_GREEN } else { 0 } | if red { LEDS_RED } else { 0 }])
}

/// 179: one LED or digital output. `led` is 6 (green), 7 (red), or on the Switch Interface 0 (Out 1) or 1 (Out 2).
/// `[0, 179, led, state, 0...]`.
pub fn set_led(led: u8, state: LightState) -> Result<[u8; OUTPUT_REPORT_LEN], LightError> {
    if !matches!(led, LED_OUT1 | LED_OUT2 | LED_GREEN | LED_RED) {
        return Err(LightError(format!("LED index {led} is not 0, 1, 6 or 7")));
    }
    Ok(report(CMD_SET_LED, &[led, state as u8]))
}

/// 180: flash frequency, 1 (fastest) to 255 (slowest, about 4 s between flashes); shared by the indicator LEDs and the backlights.
/// `[0, 180, freq, 0...]`.
pub fn set_flash_frequency(frequency: u8) -> Result<[u8; OUTPUT_REPORT_LEN], LightError> {
    if frequency == 0 {
        return Err(LightError("flash frequency 0 is not documented (1 to 255)".to_string()));
    }
    Ok(report(CMD_SET_FLASH_FREQUENCY, &[frequency]))
}

/// The backlight index of a Jog & Shuttle key for command 181: the key's Figure 1 number, plus 32 for bank 2 (`bank` 1).
pub fn backlight_index(bank: u8, key_index: u8) -> Result<u8, LightError> {
    if bank >= BANKS {
        return Err(LightError(format!("bank {bank} is not 0 or 1")));
    }
    if !JOG_SHUTTLE_KEY_INDEXES.contains(&key_index) {
        return Err(LightError(format!("key index {key_index} is not on the Jog & Shuttle (0-2, 8-10, 16-18, 24-26)")));
    }
    Ok(key_index + 32 * bank)
}

/// 181: one key's backlight in one bank. `[0, 181, index, state, 0...]`.
pub fn set_backlight(bank: u8, key_index: u8, state: LightState) -> Result<[u8; OUTPUT_REPORT_LEN], LightError> {
    Ok(report(CMD_SET_BACKLIGHT, &[backlight_index(bank, key_index)?, state as u8]))
}

/// 182: turn the backlights of a bank on or off by "row" bits (bit 0 = 1st row ... bit 5 = 6th row; a 1 bit is on, a 0 bit is off,
/// intensities are untouched). The documentation does not say which keys a "row" is on the Jog & Shuttle; the guide observes it.
/// `[0, 182, bank, mask, 0...]`.
pub fn set_backlight_rows(bank: u8, mask: u8) -> Result<[u8; OUTPUT_REPORT_LEN], LightError> {
    if bank >= BANKS {
        return Err(LightError(format!("bank {bank} is not 0 or 1")));
    }
    if mask > 0x3F {
        return Err(LightError(format!("row mask {mask:#04x} uses bits above bit 5")));
    }
    Ok(report(CMD_SET_BACKLIGHT_ROWS, &[bank, mask]))
}

/// 184: toggle the backlights. `[0, 184, 0...]`.
pub fn toggle_backlights() -> [u8; OUTPUT_REPORT_LEN] {
    report(CMD_TOGGLE_BACKLIGHTS, &[])
}

/// 187: backlight intensity of bank 1 and bank 2, 0 (off) to 255 (brightest). `[0, 187, bank1, bank2, 0...]`.
pub fn set_backlight_intensity(bank1: u8, bank2: u8) -> [u8; OUTPUT_REPORT_LEN] {
    report(CMD_SET_BACKLIGHT_INTENSITY, &[bank1, bank2])
}

/// Which lights a model has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightModel {
    JogShuttle,
    SwitchInterface,
}

/// Accept a report only if it is one of the volatile light commands above, with parameters in the documented ranges and valid for
/// `model`, and nothing but zeros after the parameters. Everything else (EEPROM writers, mode changers, unknown commands) is refused.
/// The guide's hub applies this to every report before it reaches a device.
pub fn validate_light_report(model: LightModel, r: &[u8]) -> Result<(), String> {
    if r.len() != OUTPUT_REPORT_LEN {
        return Err(format!("a light report is {OUTPUT_REPORT_LEN} bytes, not {}", r.len()));
    }
    if r[0] != 0 {
        return Err("report ID must be 0".to_string());
    }
    let (params, ok) = match (r[1], model) {
        (CMD_SET_LED, _) => {
            let led_ok = matches!(r[2], LED_GREEN | LED_RED) || (model == LightModel::SwitchInterface && matches!(r[2], LED_OUT1 | LED_OUT2));
            (2, led_ok && r[3] <= 2)
        }
        (CMD_SET_FLASH_FREQUENCY, _) => (1, r[2] >= 1),
        (CMD_SET_LEDS, LightModel::JogShuttle) => (1, r[2] & !(LEDS_GREEN | LEDS_RED) == 0),
        (CMD_SET_BACKLIGHT, LightModel::JogShuttle) => (2, r[3] <= 2 && r[2] < 64 && JOG_SHUTTLE_KEY_INDEXES.contains(&(r[2] % 32))),
        (CMD_SET_BACKLIGHT_ROWS, LightModel::JogShuttle) => (2, r[2] < BANKS && r[3] <= 0x3F),
        (CMD_TOGGLE_BACKLIGHTS, LightModel::JogShuttle) => (0, true),
        (CMD_SET_BACKLIGHT_INTENSITY, LightModel::JogShuttle) => (2, true),
        (c, _) => return Err(match NEVER_SENT.iter().find(|(n, _)| *n == c) {
            Some((_, why)) => format!("command {c} is never sent by the guide ({why})"),
            None => format!("command {c} is not a known volatile light command for this model"),
        }),
    };
    if !ok {
        return Err(format!("command {} has a parameter outside the documented range", r[1]));
    }
    if r[2 + params..].iter().any(|&b| b != 0) {
        return Err(format!("command {} has non-zero bytes after its parameters", r[1]));
    }
    Ok(())
}

/// What the documentation says about one output command, for the report and the result file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandInfo {
    pub command: u8,
    pub name: &'static str,
    /// `JogShuttle`, `SwitchInterface` or both.
    pub models: &'static str,
    /// `Some(true)` volatile (unmarked in the documentation), `Some(false)` written to EEPROM (marked "e"), `None` not stated.
    pub volatile: Option<bool>,
    /// Sent by the guide's lights part.
    pub used: bool,
    pub source: &'static str,
}

const JS_PAGE: &str = "Documentation/pages/X-keys XK-12 Jog Shuttle Data Report.htm (and ... Jog & Shuttle Data Report.htm), Output Report";
const SI_PAGE: &str = "Documentation/pages/X-keys XK-12 XK-3 Switch Interface Data Report.htm, Output Report";

/// Every output command the two pages list that matters here, with its documented persistence.
pub const COMMAND_TABLE: [CommandInfo; 17] = [
    CommandInfo { command: 186, name: "Set LEDs (green bit 0x40, red bit 0x80)", models: "Jog & Shuttle", volatile: Some(true), used: true, source: JS_PAGE },
    CommandInfo { command: 179, name: "Index Based Set LED (Flash): LED 6 green, 7 red; state 0 off, 1 on, 2 flash", models: "Jog & Shuttle, Switch Interface", volatile: Some(true), used: true, source: "both pages, item 2 (J&S) and item 1 (SI)" },
    CommandInfo { command: 179, name: "Index Based Set LED: LED 0 = Out 1, 1 = Out 2 (two digital outputs)", models: "Switch Interface", volatile: Some(true), used: true, source: SI_PAGE },
    CommandInfo { command: 180, name: "Set Frequency of Flash (1 fastest, 255 slowest)", models: "Jog & Shuttle, Switch Interface", volatile: Some(true), used: true, source: "both pages" },
    CommandInfo { command: 181, name: "Index Based Set Backlights (Flash): key index, +32 for bank 2", models: "Jog & Shuttle", volatile: Some(true), used: true, source: JS_PAGE },
    CommandInfo { command: 182, name: "Turn On/Off Rows of Backlights: bank, row bit mask", models: "Jog & Shuttle", volatile: Some(true), used: true, source: JS_PAGE },
    CommandInfo { command: 184, name: "Toggle Backlights", models: "Jog & Shuttle", volatile: Some(true), used: true, source: JS_PAGE },
    CommandInfo { command: 187, name: "Set Backlight Intensity: bank 1, bank 2, 0 to 255", models: "Jog & Shuttle", volatile: Some(true), used: true, source: JS_PAGE },
    CommandInfo { command: 183, name: "Enable Scroll Lock to Toggle Backlights", models: "Jog & Shuttle", volatile: None, used: false, source: "persistence not stated; changes a keyboard-driven mode: left out, untested" },
    CommandInfo { command: 199, name: "Save Backlight State to EEPROM", models: "Jog & Shuttle", volatile: Some(false), used: false, source: "marked e (EEPROM): never sent" },
    CommandInfo { command: 189, name: "Set Unit ID", models: "both", volatile: Some(false), used: false, source: "marked e (EEPROM): never sent" },
    CommandInfo { command: 204, name: "Change PID", models: "both", volatile: Some(false), used: false, source: "marked e (EEPROM): never sent" },
    CommandInfo { command: 238, name: "Reboot Device", models: "Switch Interface", volatile: None, used: false, source: "SI page list; identity/mode: never sent" },
    CommandInfo { command: 210, name: "Enable Time Stamp", models: "both", volatile: None, used: false, source: "persistence not stated; changes the report format: never sent" },
    CommandInfo { command: 195, name: "Set Version Number", models: "Switch Interface", volatile: Some(false), used: false, source: "marked e (EEPROM): never sent" },
    CommandInfo { command: 214, name: "Request Descriptor (would read the LED state byte)", models: "both", volatile: None, used: false, source: "not a light command; not sent by this part" },
    CommandInfo { command: 177, name: "Generate Data (used only to read key state elsewhere in the guide)", models: "both", volatile: Some(true), used: false, source: "both pages" },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(r: &[u8; OUTPUT_REPORT_LEN]) -> Vec<u8> {
        let end = r.iter().rposition(|&b| b != 0).map_or(2, |i| (i + 1).max(2));
        r[..end].to_vec()
    }

    #[test]
    fn leds_are_exactly_the_documented_bytes() {
        assert_eq!(bytes(&set_leds(true, false)), [0, 186, 0x40]);
        assert_eq!(bytes(&set_leds(false, true)), [0, 186, 0x80]);
        assert_eq!(bytes(&set_leds(true, true)), [0, 186, 0xC0]);
        assert_eq!(bytes(&set_leds(false, false)), [0, 186]);
        assert_eq!(set_leds(true, true).len(), 36);
        assert_eq!(bytes(&set_led(LED_GREEN, LightState::On).unwrap()), [0, 179, 6, 1]);
        assert_eq!(bytes(&set_led(LED_RED, LightState::Flash).unwrap()), [0, 179, 7, 2]);
        assert_eq!(bytes(&set_led(LED_OUT1, LightState::On).unwrap()), [0, 179, 0, 1]);
        assert_eq!(bytes(&set_led(LED_OUT2, LightState::Off).unwrap()), [0, 179, 1]);
        assert!(set_led(2, LightState::On).is_err());
        assert_eq!(bytes(&set_flash_frequency(40).unwrap()), [0, 180, 40]);
        assert!(set_flash_frequency(0).is_err());
    }

    #[test]
    fn backlights_use_the_figure_numbers_plus_32_for_bank_2() {
        assert_eq!(bytes(&set_backlight(0, 0, LightState::On).unwrap()), [0, 181, 0, 1]);
        assert_eq!(bytes(&set_backlight(1, 0, LightState::On).unwrap()), [0, 181, 32, 1]);
        assert_eq!(bytes(&set_backlight(0, 26, LightState::Flash).unwrap()), [0, 181, 26, 2]);
        assert_eq!(bytes(&set_backlight(1, 2, LightState::Off).unwrap()), [0, 181, 34]);
        // Figure 1: lower left key is 2, not the 5 of the page's example text.
        assert_eq!(JOG_SHUTTLE_KEY_INDEXES[8], 2);
        assert!(set_backlight(0, 3, LightState::On).is_err(), "3 is not a key of this device");
        assert!(set_backlight(2, 0, LightState::On).is_err());
        assert_eq!(bytes(&set_backlight_rows(0, 0x01).unwrap()), [0, 182, 0, 1]);
        assert_eq!(bytes(&set_backlight_rows(1, 0x3F).unwrap()), [0, 182, 1, 0x3F]);
        assert!(set_backlight_rows(0, 0x40).is_err() && set_backlight_rows(2, 1).is_err());
        assert_eq!(bytes(&toggle_backlights()), [0, 184]);
        assert_eq!(bytes(&set_backlight_intensity(255, 40)), [0, 187, 255, 40]);
        assert_eq!(bytes(&set_backlight_intensity(0, 0)), [0, 187]);
    }

    #[test]
    fn every_builder_output_validates_for_its_model_and_nothing_else_does() {
        let js = LightModel::JogShuttle;
        let si = LightModel::SwitchInterface;
        for r in [
            set_leds(true, true),
            set_led(6, LightState::On).unwrap(),
            set_led(7, LightState::Flash).unwrap(),
            set_flash_frequency(1).unwrap(),
            set_backlight(1, 24, LightState::On).unwrap(),
            set_backlight_rows(0, 0).unwrap(),
            toggle_backlights(),
            set_backlight_intensity(1, 2),
        ] {
            assert_eq!(validate_light_report(js, &r), Ok(()), "{r:?}");
        }
        // The Switch Interface has only 179 (LEDs 0, 1, 6, 7) and 180.
        assert_eq!(validate_light_report(si, &set_led(0, LightState::On).unwrap()), Ok(()));
        assert_eq!(validate_light_report(si, &set_flash_frequency(9).unwrap()), Ok(()));
        assert!(validate_light_report(si, &set_leds(true, true)).is_err());
        assert!(validate_light_report(si, &set_backlight(0, 0, LightState::On).unwrap()).is_err());
        assert!(validate_light_report(js, &set_led(0, LightState::On).unwrap()).is_err(), "the Jog & Shuttle has no digital outputs");
        // Wrong length, report ID, state, stray bytes.
        assert!(validate_light_report(js, &[0, 186, 0x40]).is_err());
        let mut bad = set_led(6, LightState::On).unwrap();
        bad[3] = 3;
        assert!(validate_light_report(js, &bad).is_err());
        let mut bad = toggle_backlights();
        bad[10] = 1;
        assert!(validate_light_report(js, &bad).is_err());
        let mut bad = set_leds(true, false);
        bad[2] |= 0x01;
        assert!(validate_light_report(js, &bad).is_err());
        let mut bad = set_backlight(0, 0, LightState::On).unwrap();
        bad[2] = 5;
        assert!(validate_light_report(js, &bad).is_err(), "the page's example index 5 is not a key in Figure 1");
    }

    #[test]
    fn eeprom_writers_and_mode_changers_are_refused_for_both_models() {
        for model in [LightModel::JogShuttle, LightModel::SwitchInterface] {
            for (cmd, _) in NEVER_SENT {
                let mut r = [0u8; OUTPUT_REPORT_LEN];
                r[1] = cmd;
                r[2] = 1;
                let e = validate_light_report(model, &r).unwrap_err();
                assert!(e.contains("never sent"), "{cmd}: {e}");
            }
            let mut r = [0u8; OUTPUT_REPORT_LEN];
            r[1] = 99;
            assert!(validate_light_report(model, &r).unwrap_err().contains("not a known volatile light command"));
        }
    }

    #[test]
    fn the_command_table_agrees_with_the_never_sent_list_and_the_builders() {
        for c in COMMAND_TABLE {
            if c.used {
                assert_eq!(c.volatile, Some(true), "{} is used so must be documented volatile", c.command);
                assert!(!NEVER_SENT.iter().any(|(n, _)| *n == c.command));
            }
            if c.volatile == Some(false) {
                assert!(!c.used, "{} writes EEPROM", c.command);
            }
        }
        for (n, _) in NEVER_SENT {
            assert!(!COMMAND_TABLE.iter().any(|c| c.used && c.command == n));
        }
        for cmd in [CMD_SET_LEDS, CMD_SET_LED, CMD_SET_FLASH_FREQUENCY, CMD_SET_BACKLIGHT, CMD_SET_BACKLIGHT_ROWS, CMD_TOGGLE_BACKLIGHTS, CMD_SET_BACKLIGHT_INTENSITY] {
            assert!(COMMAND_TABLE.iter().any(|c| c.used && c.command == cmd));
        }
    }
}
