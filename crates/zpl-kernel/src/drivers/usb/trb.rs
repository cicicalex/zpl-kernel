//! The data structures an xHCI controller reads and writes in memory, as plain numbers.
//!
//! An xHCI controller is driven almost entirely through memory: 16-byte Transfer
//! Request Blocks on rings for commands, transfers and events, and 32-byte contexts
//! that describe a device and its endpoints. This file builds and takes apart those
//! words and nothing else. It touches no memory and no register, so every bit position
//! below is checked by a test on the host, against the numbers in the xHCI 1.2
//! specification (chapter 6, "Data Structures").
//!
//! The cycle bit of a TRB is deliberately not set here: it belongs to the ring the TRB
//! is written into, and the ring sets it.

/// One TRB, before the ring gives it its cycle bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trb {
    pub param: u64,
    pub status: u32,
    pub control: u32,
}

// TRB types (bits 15:10 of the control word), xHCI 1.2 table 6-91.
pub const TYPE_NORMAL: u32 = 1;
pub const TYPE_SETUP: u32 = 2;
pub const TYPE_DATA: u32 = 3;
pub const TYPE_STATUS: u32 = 4;
pub const TYPE_LINK: u32 = 6;
pub const TYPE_ENABLE_SLOT: u32 = 9;
pub const TYPE_ADDRESS_DEVICE: u32 = 11;
pub const TYPE_CONFIGURE_ENDPOINT: u32 = 12;
pub const TYPE_EVALUATE_CONTEXT: u32 = 13;
pub const TYPE_TRANSFER_EVENT: u32 = 32;
pub const TYPE_COMMAND_COMPLETION: u32 = 33;

/// Completion codes (bits 31:24 of an event's status word), table 6-90.
pub const CC_SUCCESS: u8 = 1;
pub const CC_SHORT_PACKET: u8 = 13;

/// Control-word bits shared by several TRB types.
pub const CYCLE: u32 = 1 << 0;
/// Link TRB: toggle the consumer's cycle state when following this link.
const TOGGLE_CYCLE: u32 = 1 << 1;
/// Interrupt on Short Packet.
const ISP: u32 = 1 << 2;
/// Interrupt On Completion: produce an event when this TRB is done.
const IOC: u32 = 1 << 5;
/// Immediate Data: the parameter field holds the data itself (the setup packet).
const IDT: u32 = 1 << 6;
/// Data and status stages: bit 16 is the direction, 1 for IN.
const DIR_IN: u32 = 1 << 16;

const fn type_bits(kind: u32) -> u32 {
    kind << 10
}

/// The TRB type of a control word.
#[must_use]
pub const fn trb_type(control: u32) -> u32 {
    (control >> 10) & 0x3F
}

/// The slot an event or command names (bits 31:24 of the control word).
#[must_use]
pub const fn slot_id(control: u32) -> u8 {
    (control >> 24) as u8
}

/// The endpoint (device context index) a transfer event names (bits 20:16).
#[must_use]
pub const fn endpoint_id(control: u32) -> u8 {
    ((control >> 16) & 0x1F) as u8
}

/// The completion code of an event.
#[must_use]
pub const fn completion_code(status: u32) -> u8 {
    (status >> 24) as u8
}

/// How many bytes of a transfer were *not* moved (bits 23:0 of a transfer event).
#[must_use]
pub const fn residual_length(status: u32) -> u32 {
    status & 0x00FF_FFFF
}

/// The eight bytes of a USB control request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetupPacket {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

impl SetupPacket {
    /// Packed the way it goes on the wire, little-endian, which is the way it goes into
    /// a setup TRB's parameter field.
    #[must_use]
    pub const fn as_u64(&self) -> u64 {
        (self.request_type as u64)
            | ((self.request as u64) << 8)
            | ((self.value as u64) << 16)
            | ((self.index as u64) << 32)
            | ((self.length as u64) << 48)
    }

    /// GET_DESCRIPTOR from the device.
    #[must_use]
    pub const fn get_descriptor(kind: u8, length: u16) -> Self {
        Self { request_type: 0x80, request: 6, value: (kind as u16) << 8, index: 0, length }
    }

    /// SET_CONFIGURATION.
    #[must_use]
    pub const fn set_configuration(value: u8) -> Self {
        Self { request_type: 0x00, request: 9, value: value as u16, index: 0, length: 0 }
    }

    /// HID class request SET_PROTOCOL; protocol 0 is the boot protocol.
    #[must_use]
    pub const fn set_boot_protocol(interface: u8) -> Self {
        Self { request_type: 0x21, request: 0x0B, value: 0, index: interface as u16, length: 0 }
    }

    /// HID class request SET_IDLE with duration 0: report only when something changes.
    #[must_use]
    pub const fn set_idle_forever(interface: u8) -> Self {
        Self { request_type: 0x21, request: 0x0A, value: 0, index: interface as u16, length: 0 }
    }

    const fn is_in(&self) -> bool {
        self.request_type & 0x80 != 0
    }
}

/// Setup stage of a control transfer. The transfer type (bits 17:16) says whether a
/// data stage follows and which way: 0 none, 2 OUT, 3 IN.
#[must_use]
pub const fn setup_stage(setup: &SetupPacket) -> Trb {
    let trt = if setup.length == 0 {
        0
    } else if setup.is_in() {
        3
    } else {
        2
    };
    Trb {
        param: setup.as_u64(),
        status: 8,
        control: type_bits(TYPE_SETUP) | IDT | (trt << 16),
    }
}

/// Data stage of a control transfer, into or out of the buffer at `buffer`.
#[must_use]
pub const fn data_stage(buffer: u64, length: u16, dir_in: bool) -> Trb {
    Trb {
        param: buffer,
        status: length as u32,
        control: type_bits(TYPE_DATA) | if dir_in { DIR_IN } else { 0 },
    }
}

/// Status stage of a control transfer. Its direction is the opposite of the data
/// stage's, and IN when there was none. It asks for the one event the transfer makes.
#[must_use]
pub const fn status_stage(data_was_in: bool) -> Trb {
    Trb {
        param: 0,
        status: 0,
        control: type_bits(TYPE_STATUS) | IOC | if data_was_in { 0 } else { DIR_IN },
    }
}

/// A Normal TRB: move `length` bytes into or out of `buffer`, and say when done --
/// including when the device sent less than asked, which a keyboard always may.
#[must_use]
pub const fn normal(buffer: u64, length: u32) -> Trb {
    Trb { param: buffer, status: length, control: type_bits(TYPE_NORMAL) | IOC | ISP }
}

/// The Link TRB at the end of a ring, pointing back at its start.
#[must_use]
pub const fn link(ring_start: u64) -> Trb {
    Trb { param: ring_start, status: 0, control: type_bits(TYPE_LINK) | TOGGLE_CYCLE }
}

#[must_use]
pub const fn enable_slot() -> Trb {
    Trb { param: 0, status: 0, control: type_bits(TYPE_ENABLE_SLOT) }
}

/// Address Device, with Block Set Address clear: the controller sends SET_ADDRESS.
#[must_use]
pub const fn address_device(input_context: u64, slot: u8) -> Trb {
    Trb {
        param: input_context,
        status: 0,
        control: type_bits(TYPE_ADDRESS_DEVICE) | ((slot as u32) << 24),
    }
}

#[must_use]
pub const fn configure_endpoint(input_context: u64, slot: u8) -> Trb {
    Trb {
        param: input_context,
        status: 0,
        control: type_bits(TYPE_CONFIGURE_ENDPOINT) | ((slot as u32) << 24),
    }
}

#[must_use]
pub const fn evaluate_context(input_context: u64, slot: u8) -> Trb {
    Trb {
        param: input_context,
        status: 0,
        control: type_bits(TYPE_EVALUATE_CONTEXT) | ((slot as u32) << 24),
    }
}

// ---------------------------------------------------------------------------
// Contexts
// ---------------------------------------------------------------------------

/// Port speeds, as PORTSC bits 13:10 report them and the slot context wants them.
pub const SPEED_FULL: u8 = 1;
pub const SPEED_LOW: u8 = 2;
pub const SPEED_HIGH: u8 = 3;
pub const SPEED_SUPER: u8 = 4;

/// Endpoint types for the endpoint context (bits 5:3 of dword 1).
pub const EP_TYPE_CONTROL: u32 = 4;
pub const EP_TYPE_INTERRUPT_IN: u32 = 7;

/// The first four dwords of a slot context.
#[must_use]
pub const fn slot_context(speed: u8, context_entries: u8, root_port: u8) -> [u32; 4] {
    [
        ((speed as u32) << 20) | ((context_entries as u32) << 27),
        (root_port as u32) << 16,
        0,
        0,
    ]
}

/// The first five dwords of an endpoint context.
///
/// Error count 3 is the value the specification recommends: three tries at a
/// transaction before the endpoint is halted. The dequeue pointer carries the ring's
/// starting cycle state, 1, in bit 0.
#[must_use]
pub const fn endpoint_context(
    ep_type: u32,
    max_packet: u16,
    interval: u8,
    dequeue: u64,
    average_trb_length: u16,
    max_esit_payload: u16,
) -> [u32; 5] {
    let dequeue = dequeue | 1;
    [
        (interval as u32) << 16,
        (3 << 1) | (ep_type << 3) | ((max_packet as u32) << 16),
        (dequeue & 0xFFFF_FFFF) as u32,
        (dequeue >> 32) as u32,
        (average_trb_length as u32) | ((max_esit_payload as u32) << 16),
    ]
}

/// The maximum packet size to assume for endpoint 0 before the device has said.
///
/// Full speed may be 8, 16, 32 or 64; 8 is the size every device can do, and the
/// first eight bytes of the device descriptor -- which carry the real value -- fit in
/// it.
#[must_use]
pub const fn default_max_packet_0(speed: u8) -> u16 {
    match speed {
        SPEED_HIGH => 64,
        SPEED_SUPER => 512,
        _ => 8,
    }
}

/// The endpoint-0 packet size a device descriptor states. For SuperSpeed the byte is an
/// exponent, not a size.
#[must_use]
pub const fn max_packet_0_from_descriptor(speed: u8, b_max_packet_size_0: u8) -> u16 {
    if speed == SPEED_SUPER {
        if b_max_packet_size_0 >= 16 {
            512
        } else {
            1 << b_max_packet_size_0
        }
    } else {
        b_max_packet_size_0 as u16
    }
}

/// An endpoint's `bInterval`, in the encoding the endpoint context wants: the period is
/// 2^value units of 125 µs.
///
/// Low and full speed state the interrupt interval in 1 ms frames (1..=255), which is
/// 8 × that many microframes; the value is the largest power of two not above it,
/// clamped to the range the specification allows for them (3..=10). High and
/// SuperSpeed state it already as an exponent plus one.
#[must_use]
pub const fn interrupt_interval(speed: u8, b_interval: u8) -> u8 {
    match speed {
        SPEED_HIGH | SPEED_SUPER => {
            let v = if b_interval == 0 { 0 } else { b_interval - 1 };
            if v > 15 { 15 } else { v }
        }
        _ => {
            let frames = if b_interval == 0 { 1 } else { b_interval as u32 };
            let micro = frames * 8;
            let log2 = 31 - micro.leading_zeros();
            if log2 < 3 {
                3
            } else if log2 > 10 {
                10
            } else {
                log2 as u8
            }
        }
    }
}

/// The device context index of an IN endpoint: twice its number, plus one.
#[must_use]
pub const fn dci_for_in_endpoint(address: u8) -> u8 {
    (address & 0x0F) * 2 + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_get_descriptor_setup_packet_has_the_wire_layout() {
        let s = SetupPacket::get_descriptor(1, 18);
        // 80 06 00 01 00 00 12 00
        assert_eq!(s.as_u64().to_le_bytes(), [0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 0x12, 0x00]);
    }

    #[test]
    fn the_setup_stage_says_which_way_the_data_goes() {
        let get = setup_stage(&SetupPacket::get_descriptor(1, 8));
        assert_eq!(trb_type(get.control), TYPE_SETUP);
        assert_eq!((get.control >> 16) & 3, 3, "IN data stage");
        assert_eq!(get.status, 8, "a setup packet is eight bytes");
        assert_ne!(get.control & IDT, 0, "the packet is in the TRB itself");
        let set = setup_stage(&SetupPacket::set_configuration(1));
        assert_eq!((set.control >> 16) & 3, 0, "no data stage");
    }

    #[test]
    fn the_status_stage_goes_the_other_way_from_the_data() {
        assert_eq!(status_stage(true).control & DIR_IN, 0);
        assert_ne!(status_stage(false).control & DIR_IN, 0);
        assert_ne!(status_stage(true).control & IOC, 0);
    }

    #[test]
    fn no_builder_sets_the_cycle_bit() {
        for t in [
            enable_slot(),
            address_device(0x1000, 1),
            configure_endpoint(0x1000, 1),
            evaluate_context(0x1000, 1),
            normal(0x2000, 8),
            link(0x3000),
            setup_stage(&SetupPacket::get_descriptor(1, 8)),
            data_stage(0x2000, 8, true),
            status_stage(true),
        ] {
            assert_eq!(t.control & CYCLE, 0);
        }
    }

    #[test]
    fn commands_carry_their_type_and_slot() {
        let a = address_device(0xABC0, 5);
        assert_eq!(trb_type(a.control), TYPE_ADDRESS_DEVICE);
        assert_eq!(slot_id(a.control), 5);
        assert_eq!(a.param, 0xABC0);
        assert_eq!(trb_type(configure_endpoint(0, 1).control), TYPE_CONFIGURE_ENDPOINT);
        assert_eq!(trb_type(evaluate_context(0, 1).control), TYPE_EVALUATE_CONTEXT);
        assert_eq!(trb_type(enable_slot().control), TYPE_ENABLE_SLOT);
    }

    #[test]
    fn the_link_trb_toggles_the_cycle() {
        let l = link(0x5000);
        assert_eq!(trb_type(l.control), TYPE_LINK);
        assert_ne!(l.control & TOGGLE_CYCLE, 0);
    }

    #[test]
    fn event_fields_are_read_from_where_the_specification_puts_them() {
        // A transfer event: slot 2, endpoint 3, short packet, 5 bytes not moved.
        let control = (2u32 << 24) | (3 << 16) | (TYPE_TRANSFER_EVENT << 10) | 1;
        let status = (u32::from(CC_SHORT_PACKET) << 24) | 5;
        assert_eq!(slot_id(control), 2);
        assert_eq!(endpoint_id(control), 3);
        assert_eq!(trb_type(control), TYPE_TRANSFER_EVENT);
        assert_eq!(completion_code(status), CC_SHORT_PACKET);
        assert_eq!(residual_length(status), 5);
    }

    #[test]
    fn a_slot_context_puts_speed_entries_and_port_in_place() {
        let s = slot_context(SPEED_FULL, 3, 7);
        assert_eq!((s[0] >> 20) & 0xF, 1);
        assert_eq!(s[0] >> 27, 3);
        assert_eq!((s[1] >> 16) & 0xFF, 7);
    }

    #[test]
    fn an_endpoint_context_puts_every_field_in_place() {
        let e = endpoint_context(EP_TYPE_INTERRUPT_IN, 8, 6, 0x1_2345_6000, 8, 8);
        assert_eq!((e[0] >> 16) & 0xFF, 6, "interval");
        assert_eq!((e[1] >> 1) & 3, 3, "error count");
        assert_eq!((e[1] >> 3) & 7, EP_TYPE_INTERRUPT_IN);
        assert_eq!(e[1] >> 16, 8, "max packet");
        assert_eq!(e[2], 0x2345_6001, "dequeue low, with the cycle state");
        assert_eq!(e[3], 1, "dequeue high");
        assert_eq!(e[4], 8 | (8 << 16));
    }

    #[test]
    fn interrupt_intervals_are_converted_per_speed() {
        // Full speed, 10 ms: 80 microframes, largest power of two below is 64 = 2^6.
        assert_eq!(interrupt_interval(SPEED_FULL, 10), 6);
        assert_eq!(interrupt_interval(SPEED_FULL, 1), 3);
        assert_eq!(interrupt_interval(SPEED_LOW, 255), 10);
        assert_eq!(interrupt_interval(SPEED_FULL, 0), 3);
        // High speed states an exponent plus one.
        assert_eq!(interrupt_interval(SPEED_HIGH, 7), 6);
        assert_eq!(interrupt_interval(SPEED_HIGH, 0), 0);
        assert_eq!(interrupt_interval(SPEED_SUPER, 40), 15);
    }

    #[test]
    fn endpoint_zero_packet_sizes() {
        assert_eq!(default_max_packet_0(SPEED_LOW), 8);
        assert_eq!(default_max_packet_0(SPEED_FULL), 8);
        assert_eq!(default_max_packet_0(SPEED_HIGH), 64);
        assert_eq!(default_max_packet_0(SPEED_SUPER), 512);
        assert_eq!(max_packet_0_from_descriptor(SPEED_FULL, 64), 64);
        assert_eq!(max_packet_0_from_descriptor(SPEED_SUPER, 9), 512);
    }

    #[test]
    fn in_endpoint_one_is_context_three() {
        assert_eq!(dci_for_in_endpoint(0x81), 3);
        assert_eq!(dci_for_in_endpoint(0x82), 5);
    }
}
