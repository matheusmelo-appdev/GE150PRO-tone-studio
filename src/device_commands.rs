//! GE150 command and response IDs reverse-engineered from the official editor.

pub const READ_DEVICE_SYSTEM_DATA: u8 = 0x31;
pub const DEVICE_SYSTEM_DATA_RESPONSE: u8 = 0x32;
pub const ALL_PRESETS: u8 = 0xa0;
pub const PRESET_RECORD: u8 = 0x20;
pub const CUSTOM_MODEL_NAMES_RESPONSE: u8 = 0x41;
pub const READ_CUSTOM_MODEL_NAMES: u8 = 0xc1;
pub const CUSTOM_MODEL_STATUS_RESPONSE: u8 = 0x4d;
pub const READ_RHYTHM_DATA: u8 = 0x8c;
pub const RHYTHM_DATA_RESPONSE: u8 = 0x0c;
pub const AMP_IMPORT_PACKET: u8 = 0xd3;
pub const IR_IMPORT_PACKET: u8 = 0xb5;
pub const IMPORT_ACK: u8 = 0x15;
pub const ACTIVATE_PRESET: u8 = 0x96;
pub const MODULE_UPDATE_BASE: u8 = 0x82;
pub const WRITE_PRESET: u8 = 0xc3;
