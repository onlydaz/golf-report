// Địa chỉ Modbus PDU (zero-based), không phải ký hiệu 4xxxx của một số PLC.
pub const START: u16 = 100;
pub const COUNT: usize = 18;
pub const COMPLETED: usize = 0;
pub const SEQUENCE: usize = 1;
pub const CLUB: usize = 2;
pub const WEIGHT: usize = 3;
pub const TIP_DISTANCE: usize = 4;
pub const TIP_RETRY: usize = 8;
pub const TIP_RESULT: usize = 12;
pub const BUTT_DISTANCE: usize = 16;
pub const BUTT_RESULT: usize = 17;
