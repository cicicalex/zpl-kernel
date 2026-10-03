//! The boot path: everything between the first instruction of the kernel and
//! the halt loop.
//!
//! `kernel_entry` runs the self-checks in order -- memory, paging, rings,
//! filesystem, IPC, shared memory, the scheduler -- and each one announces
//! itself on COM1 with a one-line marker. `docs/MARKERS.md` lists every marker
//! and what it means; `emit_critical_marker` is the one place a marker is
//! written, which is why it is also the one place that mirrors to the screen.
use crate::drivers::serial::{
    apply_sink_lane_preset, set_boot_log_sink_mode, BootLogLanePreset, BootLogSinkMode,
    init_early_serial, set_boot_log_ring_write_mode, BootLogRingWriteMode,
};

#[derive(Debug, Clone, Copy)]
pub struct BootInfo {
    pub boot_magic: u64,
    pub memory_map_entries: u32,
    pub memory_bytes: u64,
    pub cpu_count: u32,
}

impl BootInfo {
    pub const fn minimal() -> Self {
        Self {
            boot_magic: 0,
            memory_map_entries: 0,
            memory_bytes: 64 * 1024 * 1024,
            cpu_count: 1,
        }
    }
}

// The output primitives live in `drivers::console` now: everything in the kernel
// needs them, and having them here is what made eight modules depend on `boot`.
use crate::drivers::console::{boot_probe_byte, emit_critical_marker, with_irq_masked};
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
use crate::drivers::console::MarkerLine;

pub fn kernel_entry(_info: BootInfo) -> ! {
    prepare_screen();
    early_console();
    memory_and_core_self_checks();
    enter_ring3_and_interrupts();
    subsystem_self_checks();
    probe_devices();

    // #endregion agent log
    // The demo configuration emits this from `demo_programs::finish` instead,
    // after the programs have run, so the line still lands where the boot ends.
    #[cfg(all(
        any(
            not(all(target_os = "none", target_arch = "x86_64")),
            all(not(feature = "ring3_demo"), not(feature = "elf_demo"))
        ),
        not(all(
            target_os = "none",
            target_arch = "x86_64",
            feature = "qemu_boot",
            feature = "vga_crit_mirror",
            not(feature = "panic_test")
        ))
    ))]
    static MARKER3: &[u8] = b"[ZPL-BOOT] halt loop entered\n";

    #[cfg(all(
        target_os = "none",
        target_arch = "x86_64",
        feature = "ring3_demo"
    ))]
    run_ring3_demo();

    #[cfg(all(
        target_os = "none",
        target_arch = "x86_64",
        feature = "elf_demo"
    ))]
    run_elf_demo();

    // The public `-kernel` build runs its three ring-3 demo programs here instead
    // of idling: everything the kernel needs is up, and the per-boot quota still
    // has room, which program 3 needs in order to reach it itself. The call does
    // not return -- the programs end with `SYS_EXIT`, and `demo_programs::finish`
    // emits the timing lines and the halt marker from inside that handler.
    #[cfg(all(
        target_os = "none",
        target_arch = "x86_64",
        feature = "qemu_boot",
        feature = "vga_crit_mirror",
        not(feature = "ring3_demo"),
        not(feature = "elf_demo"),
        not(feature = "panic_test")
    ))]
    crate::demo::demo_programs::run_then_halt();

    #[cfg(all(
        any(
            not(all(target_os = "none", target_arch = "x86_64")),
            all(not(feature = "ring3_demo"), not(feature = "elf_demo"))
        ),
        not(all(
            target_os = "none",
            target_arch = "x86_64",
            feature = "qemu_boot",
            feature = "vga_crit_mirror",
            not(feature = "panic_test")
        ))
    ))]
    {
        #[cfg(all(target_os = "none", target_arch = "x86_64"))]
        crate::audit::timing::checkpoint(crate::audit::timing::Phase::HaltLoop);

        #[cfg(all(target_os = "none", target_arch = "x86_64"))]
        with_irq_masked(|| {
            // The per-phase timing lines stay serial-only: sixteen rows of rdtsc
            // counters would push the self-checks and the decisions off a 25-row
            // screen, and `v04_status::line_counts` already keeps them out of the
            // hashed set for the same reason — they are not behaviour.
            crate::audit::timing::emit(boot_probe_byte);
        });
        #[cfg(all(target_os = "none", target_arch = "x86_64"))]
        run_e1000_ask();

        emit_critical_marker(MARKER3);

        #[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
        for byte in MARKER3.iter() {
            boot_probe_byte(*byte);
        }

        #[cfg(feature = "panic_test")]
        {
            panic!("test");
        }

        #[cfg(not(feature = "panic_test"))]
        {
            // #region agent log
            #[cfg(all(
                target_os = "none",
                target_arch = "x86_64",
                feature = "agent_debug_boot"
            ))]
            crate::agent_debug_com1::ndjson_line(crate::agent_debug_com1::DBG_PRE_VISUAL_HOLD);
            // #endregion agent log
            #[cfg(all(
                target_os = "none",
                target_arch = "x86_64",
                feature = "vga_visual_hold",
            ))]
            run_visual_alive_loop();

            #[cfg(not(all(
                target_os = "none",
                target_arch = "x86_64",
                feature = "vga_visual_hold",
            )))]
            loop {
                crate::arch::x86_64::halt_once();
            }
        }
    }
}

/// Put the screen in a known state before anything is printed on it.
///
/// Only the `-kernel` path clears the text buffer: there 0xB8000 is identity-mapped
/// from the first instruction, so the write cannot fault. The Limine path must not,
/// and `vga::ensure_init` says why.
///
/// The panel is drawn once here so the screen has its shape from the first frame
/// rather than growing one seven rows in.
fn prepare_screen() {
    // NOTE (M1): we deliberately skip `reset_boot_log_state()` on first boot
    // because BSS is already zero-initialised and the legacy reset path was
    // observed to busy-spin in early boot before paging is set up.

    // Clear the text buffer once, so the screen shows this kernel's output and not
    // whatever SeaBIOS left behind. Only on the `qemu_boot` path: there 0xB8000 is
    // identity-mapped from the first instruction, so the write cannot fault. The
    // Limine path deliberately does NOT clear -- see `vga::ensure_init` for why a
    // full-screen write there can cascade to a triple fault.
    #[cfg(all(
        target_os = "none",
        target_arch = "x86_64",
        feature = "vga_crit_mirror",
        feature = "qemu_boot"
    ))]
    crate::drivers::vga::clear();

    // Draw the panel once before any decision happens, so the screen has its
    // shape from the first frame instead of growing one seven rows in.
    #[cfg(all(
        target_os = "none",
        target_arch = "x86_64",
        any(
            all(feature = "qemu_boot", feature = "vga_crit_mirror"),
            feature = "gate_panel_fb"
        )
    ))]
    crate::ui::gate_panel::draw();

    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    set_boot_log_sink_mode(BootLogSinkMode::MirrorCom1);
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    set_boot_log_ring_write_mode(BootLogRingWriteMode::SourceShardedLockFree);
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    apply_sink_lane_preset(BootLogLanePreset::ContextAndHealth);
    #[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
    set_boot_log_sink_mode(BootLogSinkMode::BufferOnly);
    #[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
    set_boot_log_ring_write_mode(BootLogRingWriteMode::GlobalLock);
    #[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
    apply_sink_lane_preset(BootLogLanePreset::All);

    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    unsafe {
        crate::interrupts::install_early_idt();
    }

    // #region agent log
    #[cfg(all(
        target_os = "none",
        target_arch = "x86_64",
        feature = "agent_debug_boot"
    ))]
    crate::agent_debug_com1::ndjson_line(crate::agent_debug_com1::DBG_PRE_MARKER1);
}

/// The first two things the kernel says, and the port it says them on.
///
/// Announces itself, brings up COM1, announces that, and records the build identity.
/// Nothing here can fail usefully: if it does, there is no way to report it, which
/// is why it is the shortest phase and the first.
fn early_console() {
    static MARKER1: &[u8] = b"[ZPL-BOOT] kernel_entry reached\n";
    static MARKER2: &[u8] = b"[ZPL-BOOT] serial initialized\n";
    emit_critical_marker(MARKER1);
    // #region agent log
    #[cfg(all(
        target_os = "none",
        target_arch = "x86_64",
        feature = "agent_debug_boot"
    ))]
    crate::agent_debug_com1::ndjson_line(crate::agent_debug_com1::DBG_POST_MARKER1);
    // #endregion agent log
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    crate::audit::timing::checkpoint(crate::audit::timing::Phase::KernelEntry);
    init_early_serial();
    emit_critical_marker(MARKER2);
    // #region agent log
    #[cfg(all(
        target_os = "none",
        target_arch = "x86_64",
        feature = "agent_debug_boot"
    ))]
    crate::agent_debug_com1::ndjson_line(crate::agent_debug_com1::DBG_POST_SERIAL);
    // #endregion agent log
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    crate::audit::timing::checkpoint(crate::audit::timing::Phase::SerialReady);

    with_irq_masked(|| {
        crate::audit::trace_event::emit_event(
            "BUILD-ID",
            0,
            &[
                crate::audit::trace_event::EventAttr { key: "kind", value: "kernel_image" },
                crate::audit::trace_event::EventAttr { key: "version", value: crate::KERNEL_VERSION },
            ],
        );
    });

}

/// Frames, heap, the decision log, the CPU topology, and paging -- in that order,
/// because each one needs the one before it.
///
/// Split out of `kernel_entry` without reordering anything: the markers these emit,
/// and the order they emit them in, are what the boot smoke test reads.
fn memory_and_core_self_checks() {
    run_frame_alloc_self_check();
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    crate::audit::timing::checkpoint(crate::audit::timing::Phase::FrameAlloc);

    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    run_heap_self_check();
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    crate::audit::timing::checkpoint(crate::audit::timing::Phase::Heap);

    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    run_audit_chain_self_check();
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    crate::audit::timing::checkpoint(crate::audit::timing::Phase::AuditChain);

    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    run_smp_probe();
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    crate::audit::timing::checkpoint(crate::audit::timing::Phase::Smp);

    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    run_paging_self_check();
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    crate::audit::timing::checkpoint(crate::audit::timing::Phase::Paging);

    // #region agent log
    #[cfg(all(
        target_os = "none",
        target_arch = "x86_64",
        feature = "agent_debug_boot"
    ))]
    crate::agent_debug_com1::ndjson_line(crate::agent_debug_com1::DBG_POST_PAGING_SC);
    // #endregion agent log

}

/// The task-state segment, then the interrupt table and the timer.
///
/// These two are a phase of their own because everything after them can be
/// interrupted, and everything before them cannot.
fn enter_ring3_and_interrupts() {
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    install_tss_for_ring3();
    // Phase 7 (`ring3`) had no checkpoint, so it was the one gap in the
    // `[ZPL-PERF]` series: the enum reserved the slot and nothing ever wrote it.
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    crate::audit::timing::checkpoint(crate::audit::timing::Phase::Ring3);

    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    unsafe {
        crate::interrupts::init();
    }
    // #region agent log
    #[cfg(all(
        target_os = "none",
        target_arch = "x86_64",
        feature = "agent_debug_boot"
    ))]
    crate::agent_debug_com1::ndjson_line(crate::agent_debug_com1::DBG_POST_IRQ_INIT);
    // #endregion agent log

}

/// Everything that needed interrupts running: the page-fault demo, the stack guard,
/// the filesystem, processes, pipes, shared memory, the matrix operators, the run
/// queue and the hardware abstraction layer.
fn subsystem_self_checks() {
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    run_paging_pf_demo();

    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    run_stack_guard_demo();

    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    run_ramfs_self_check();
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    crate::audit::timing::checkpoint(crate::audit::timing::Phase::Ramfs);

    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    run_process_self_check();
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    crate::audit::timing::checkpoint(crate::audit::timing::Phase::Process);

    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    run_ipc_self_check();
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    crate::audit::timing::checkpoint(crate::audit::timing::Phase::Ipc);

    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    run_shm_self_check();
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    crate::audit::timing::checkpoint(crate::audit::timing::Phase::Shm);

    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    run_matrix_ops_self_check();
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    crate::audit::timing::checkpoint(crate::audit::timing::Phase::Matrix);

    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    run_runqueue_self_check();
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    crate::audit::timing::checkpoint(crate::audit::timing::Phase::Runqueue);

    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    run_hal_self_check();
    // #region agent log
    #[cfg(all(
        target_os = "none",
        target_arch = "x86_64",
        feature = "agent_debug_boot"
    ))]
    crate::agent_debug_com1::ndjson_line(crate::agent_debug_com1::DBG_POST_HAL);
    // #endregion agent log
    // The card is set up here, near the top of the boot, and asked its question
    // at the bottom. Not tidiness: enabling the receiver starts a timer in the
    // emulated card during which it accepts nothing, so everything the kernel
    // does in between is time the question no longer has to wait for.
}

/// Walk the PCI bus and set up the network card.
///
/// Last, and deliberately: enabling the receiver starts a timer in the emulated card
/// during which it accepts nothing, so everything done before this is time the
/// card's answer no longer has to wait for. See the note in the body.
fn probe_devices() {
    // `skip_pci` is a diagnostic build only. A keyboard reaching the kernel through the
    // firmware's legacy USB emulation stopped working somewhere after boot, and walking
    // the bus is the one thing this kernel does that could plausibly disturb firmware
    // that owns the same devices. Skipping it is the cheapest way to ask.
    #[cfg(all(target_os = "none", target_arch = "x86_64", not(feature = "skip_pci")))]
    run_pci_enumeration();
    #[cfg(all(target_os = "none", target_arch = "x86_64", feature = "skip_pci"))]
    emit_critical_marker(b"[ZPL-PCI] enumeration skipped (skip_pci build)\n");
    #[cfg(all(target_os = "none", target_arch = "x86_64", not(feature = "skip_pci")))]
    run_virtio_net_probe();
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    crate::audit::timing::checkpoint(crate::audit::timing::Phase::Hal);
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn install_tss_for_ring3() {
    static MARKER_TSS: &[u8] = b"[ZPL-RING] tss installed selector=0x28\n";
    let rsp0 = current_rsp();
    unsafe {
        crate::tss::init(rsp0);
    }
    emit_critical_marker(MARKER_TSS);
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
#[inline(always)]
fn current_rsp() -> u64 {
    let value: u64;
    unsafe {
        core::arch::asm!(
            "mov {0}, rsp",
            out(reg) value,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}

#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    feature = "ring3_demo"
))]
fn run_ring3_demo() -> ! {
    static MARKER_PREP: &[u8] = b"[ZPL-RING] ring3 prepare ok\n";
    static MARKER_ENTER: &[u8] = b"[ZPL-RING] kernel->ring3 iretq triggered\n";
    static MARKER_FAIL: &[u8] = b"[ZPL-RING] ring3 prepare FAIL\n";

    unsafe {
        core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
    }

    if unsafe { crate::demo::ring3_demo::prepare() }.is_err() {
        for byte in MARKER_FAIL.iter() {
            boot_probe_byte(*byte);
        }
        crate::qemu_exit::qemu_exit_failure(2);
    }
    for byte in MARKER_PREP.iter() {
        boot_probe_byte(*byte);
    }
    for byte in MARKER_ENTER.iter() {
        boot_probe_byte(*byte);
    }

    unsafe {
        core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
        crate::demo::ring3_demo::enter_ring3();
    }
}

#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    feature = "elf_demo"
))]
fn run_elf_demo() -> ! {
    static MARKER_ENTER: &[u8] = b"[ZPL-ELF] demo entered\n";
    static MARKER_LOAD_OK: &[u8] = b"[ZPL-ELF] load ok\n";
    static MARKER_FAIL: &[u8] = b"[ZPL-ELF] load FAIL\n";

    emit_critical_marker(MARKER_ENTER);
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
    }

    let entry = match unsafe { crate::demo::elf_demo::prepare() } {
        Ok(entry) => entry,
        Err(_) => {
            for byte in MARKER_FAIL.iter() {
                boot_probe_byte(*byte);
            }
            crate::qemu_exit::qemu_exit_failure(3);
        }
    };
    for byte in MARKER_LOAD_OK.iter() {
        boot_probe_byte(*byte);
    }
    write_elf_entry_marker(entry);

    unsafe {
        core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
    }

    // The acceptance test for the ring-3 round trip. `enter_ring3` never comes back:
    // the program calls `zpl_exit` and the machine stops, which is what this path has
    // always done. `user_run::enter` comes back with the exit code, which is what a
    // prompt needs -- so this feature runs the same image the other way and says what
    // it got, on the boot path where a rebuild takes twenty seconds instead of two
    // minutes.
    #[cfg(feature = "user_run_demo")]
    {
        // SAFETY: `prepare()` above mapped the image and the user stack, the TSS is
        // installed, and nothing else is in ring 3.
        match unsafe { crate::demo::user_run::enter(entry) } {
            Ok(code) => {
                let mut line = MarkerLine::new();
                line.push(b"[ZPL-USERRUN] returned from ring 3, exit code=");
                line.push_dec(code);
                line.push(b"
");
                emit_critical_marker(line.as_slice());
            }
            Err(_) => emit_critical_marker(b"[ZPL-USERRUN] could not enter FAIL
"),
        }
        emit_critical_marker(b"[ZPL-USERRUN] the kernel is still here
");
        crate::qemu_exit::qemu_exit_success();
    }

    #[cfg(not(feature = "user_run_demo"))]
    unsafe {
        crate::demo::elf_demo::enter_ring3(entry);
    }
}

#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    feature = "elf_demo"
))]
fn write_elf_entry_marker(entry: u64) {
    static PREFIX: &[u8] = b"[ZPL-ELF] entry=0x";
    static SUFFIX: &[u8] = b"\n";
    for byte in PREFIX.iter() {
        boot_probe_byte(*byte);
    }
    for shift in (0..64).step_by(4).rev() {
        let nibble = ((entry >> shift) & 0xF) as u8;
        let c = if nibble < 10 {
            b'0' + nibble
        } else {
            b'a' + nibble - 10
        };
        boot_probe_byte(c);
    }
    for byte in SUFFIX.iter() {
        boot_probe_byte(*byte);
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn run_paging_self_check() {
    static MARKER_MAP: &[u8] = b"[ZPL-PAGING] map_4k virt=0x40000000 ok\n";
    static MARKER_VERIFY: &[u8] = b"[ZPL-PAGING] write_read_match val=0xcafebabedeadbeef\n";
    static MARKER_UNMAP: &[u8] = b"[ZPL-PAGING] unmap virt=0x40000000 ok\n";
    static MARKER_FAIL: &[u8] = b"[ZPL-PAGING] selfcheck FAIL\n";

    unsafe {
        crate::mm::paging::init();
    }
    match unsafe { crate::mm::paging::self_check_map_write_read_unmap() } {
        Ok(_) => {
            emit_critical_marker(MARKER_MAP);
            emit_critical_marker(MARKER_VERIFY);
            emit_critical_marker(MARKER_UNMAP);
        }
        Err(_) => {
            emit_critical_marker(MARKER_FAIL);
        }
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn run_hal_self_check() {
    static MARKER_OK: &[u8] = b"[ZPL-HAL] selfcheck block_rw+net_tx_rx OK\n";
    static MARKER_FAIL: &[u8] = b"[ZPL-HAL] selfcheck FAIL\n";

    match crate::hal::self_check() {
        Ok(_) => {
            emit_critical_marker(MARKER_OK);
        }
        Err(_) => {
            emit_critical_marker(MARKER_FAIL);
        }
    }
}


/// Wait briefly for an ARP reply and report it.
///
/// The receiver is set up before the request goes out, because a reply can come
/// back faster than the kernel can finish printing that it asked.
///
/// The budget is eight thousand of the units `rx_poll` counts in, which is a
/// floor-clock millisecond each. It is not sized for the network: on the wire
/// the reply is there in well under a millisecond. It is sized for a timer in
/// the device model, which accepts nothing for about a second after the
/// receiver is enabled. This kernel enables the receiver and asks its question
/// microseconds apart, so it pays that second in full. The measurements that
/// pin it to the enabling write are next to the loop in `e1000::rx_poll`.
///
/// The waiting itself is cheap: the loop halts until an interrupt wakes it, so
/// it costs about a hundred scans rather than a spinning core.
///
/// Bounded on purpose: nothing on the machine is obliged to answer, and a
/// boot that stalls waiting for a network that is not there would be a worse
/// kernel than one that says "nobody answered". A miss is a normal outcome,
/// not a failure, and the boot carries on either way.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn report_arp_reply(rx: &crate::drivers::e1000::RxRing) {
    use crate::drivers::e1000;

    let out = {
        // SAFETY: `rx` came from `rx_init` and the register window is still mapped.
        unsafe { e1000::rx_poll(rx, 8_000) }
    };

    // The same two numbers on both paths, because a wait that found nothing is
    // only informative if it says what it spent. `cycles` is raw: this kernel
    // does not know the clock rate, and a number it cannot convert honestly is
    // better printed than converted with a guess.
    // When the card and its PHY said they were ready, next to when a frame
    // actually turned up. Written as its own line so the two can be compared
    // without either being a guess about the other.
    with_irq_masked(|| {
        let mut line = MarkerLine::new();
        line.push(b"[ZPL-E1000] link status_lu=");
        push_moment(&mut line, out.link.status_lu);
        line.push(b" phy_link=");
        push_moment(&mut line, out.link.phy_link);
        line.push(b" phy_autoneg=");
        push_moment(&mut line, out.link.phy_autoneg);
        line.push(b" mii=0x");
        push_hex4(&mut line, out.link.phy_first_word);
        line.push(b" phy_unreadable=");
        line.push_dec(u64::from(out.link.phy_unreadable));
        line.push(b" frame_at=");
        line.push_dec(out.cycles);
        line.push(b"\n");
        emit_critical_marker(line.as_slice());
    });

    let tail = |line: &mut MarkerLine| {
        line.push(b" polls=");
        line.push_dec(out.polls);
        line.push(b" irqs=");
        line.push_dec(u64::from(e1000::rx_irq_count()));
        line.push(b" icr=0x");
        push_hex8(line, e1000::last_icr());
        line.push(b" cycles=");
        line.push_dec(out.cycles);
        line.push(b" budget=");
        line.push_dec(out.budget);
    };

    let Some(frame) = out.frame else {
        with_irq_masked(|| {
            let mut line = MarkerLine::new();
            line.push(b"[ZPL-E1000] rx no reply");
            tail(&mut line);
            line.push(b" (nothing is obliged to answer)\n");
            emit_critical_marker(line.as_slice());
        });
        return;
    };

    let is_reply = e1000::is_arp_reply_for_us(&frame.bytes[..frame.len.min(frame.bytes.len())]);

    with_irq_masked(|| {
        let mut line = MarkerLine::new();
        line.push(b"[ZPL-E1000] rx len=");
        line.push_dec(frame.len as u64);
        tail(&mut line);
        line.push(b" from=");
        for i in 0..6 {
            if i > 0 {
                line.push(b":");
            }
            push_hex2(&mut line, frame.bytes[6 + i]);
        }
        if is_reply {
            line.push(b" arp reply: 10.0.2.2 is at that address OK\n");
        } else {
            line.push(b" not an arp reply for us\n");
        }
        emit_critical_marker(line.as_slice());
    });
}

/// Send one ARP request from the e1000, and say what the card reported.
///
/// The threshold this was aiming at: the kernel puts a frame on the wire that
/// somebody outside the machine can see. Everything up to here could be checked
/// only by believing the kernel's own log; this cannot.
///
/// Three firsts in one function, and each can fail on its own: the first write
/// to a device's configuration space (bus mastering), the first memory the
/// kernel hands to a device to read by itself (the descriptor ring), and the
/// first time it waits for hardware to report back.
// Both boot paths now. It used to be `qemu_boot` only: the driver hung the boot
// on the Limine path, for the same reason the ELF loader did -- physical addresses
// used as virtual ones. Measured with markers, then fixed in the driver.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn run_e1000_arm(scan: &crate::drivers::pci::PciScan, mac: &[u8; 6]) {
    use crate::drivers::e1000;

    static NO_CARD: &[u8] = b"[ZPL-E1000] tx skipped, no card\n";
    static INIT_FAIL: &[u8] = b"[ZPL-E1000] tx ring init FAIL\n";

    let Some(card) = scan.devices.iter().flatten().find(|d| e1000::is_e1000(d)) else {
        emit_critical_marker(NO_CARD);
        return;
    };

    // SAFETY: `card` is the e1000 the enumeration found, so this writes the
    // command register of that card and no other.
    unsafe { e1000::enable_bus_master(card) };

    // SAFETY: `run_e1000_mac_read` mapped the register window earlier in the
    // boot, and bus mastering was just turned on, which is what `tx_init`
    // requires before the card will read the ring it is given.
    let tx = match unsafe { e1000::tx_init() } {
        Ok(r) => r,
        Err(_) => {
            emit_critical_marker(INIT_FAIL);
            return;
        }
    };

    // Enabling the receiver is what starts the device model's timer, so it
    // happens here, as early in the boot as the card is known -- and the
    // question waits until the end. See `e1000::rx_poll` for the measurements
    // that make this the right order rather than a preference.
    // SAFETY: same contract as `tx_init` above -- window mapped, bus
    // mastering on.
    let rx = unsafe { e1000::rx_init() }.ok();

    // And let the card say so itself, rather than only being asked. The order
    // matters: a handler, then the line unmasked, then the card told it may
    // raise it. Reversed, the card could assert a line nobody acknowledges.
    if rx.is_some() {
        // SAFETY: `interrupts::init` ran long before this and installed the
        // handler for IRQ 11; the register window is mapped for as long as the
        // ring is alive.
        unsafe {
            crate::interrupts::unmask_nic_irq();
            e1000::enable_rx_interrupt();
        }
    }

    e1000::store_armed(e1000::Armed { tx, rx, mac: *mac });
}

/// Ask the question, at the end of the boot.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub(crate) fn run_e1000_ask() {
    let Some(ready) = crate::drivers::e1000::armed() else {
        return;
    };
    ask_once(&ready.tx, ready.rx.as_ref(), &ready.mac);

    #[cfg(feature = "net_console")]
    net_console(&ready.tx, ready.rx.as_ref(), &ready.mac);
}

/// A moment from [`e1000::LinkWatch`]: a cycle count, or the word for one of
/// the two ends of the range. Printing `0` for "it was already true" and
/// `18446744073709551615` for "it never was" would be numbers that read as
/// measurements and are not.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn push_moment(line: &mut MarkerLine, moment: u64) {
    use crate::drivers::e1000::LinkWatch;
    match moment {
        LinkWatch::ALREADY => line.push(b"already"),
        LinkWatch::NEVER => line.push(b"never"),
        cycles => line.push_dec(cycles),
    }
}

/// A key on COM1 sends a request, so the demo is not only what boot does.
///
/// This is **not** the interactive shell and is not a step towards it: there is
/// no line editing, no parser and no command table. One key, one packet. The
/// point is to show a request leaving the card at a moment somebody chose, and
/// what that changes: asked at boot the reply takes about 2.3 s to become
/// visible, asked from here it is there on the first scan, because by then the
/// emulated card has started delivering. Same code, same wire, different
/// moment.
///
/// Reading never blocks, and the loop still ends on its own: `q`, or a bounded
/// number of idle turns, so a run with nothing typed finishes instead of
/// hanging.
#[cfg(all(target_os = "none", target_arch = "x86_64", feature = "net_console"))]
fn net_console(
    ring: &crate::drivers::e1000::TxRing,
    rx: Option<&crate::drivers::e1000::RxRing>,
    mac: &[u8; 6],
) {
    static HELP: &[u8] =
        b"[ZPL-NET] press a to ask who has 10.0.2.2, q to stop\n";
    static BYE: &[u8] = b"[ZPL-NET] console closed\n";
    static IDLE: &[u8] = b"[ZPL-NET] nothing typed, closing the console\n";

    emit_critical_marker(HELP);

    // About thirty seconds of waiting at the floor clock, then it gives up. A
    // demo that waits forever is a demo that hangs somebody else's automation.
    const GIVE_UP: u64 = 1_500_000_000 * 30;
    // SAFETY: reading the cycle counter.
    let started = unsafe { core::arch::x86_64::_rdtsc() };

    loop {
        // SAFETY: reading the cycle counter.
        let spent = unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(started);
        if spent >= GIVE_UP {
            emit_critical_marker(IDLE);
            return;
        }
        match crate::drivers::serial::read_com1_byte() {
            Some(b'a') => ask_once(ring, rx, mac),
            Some(b'q') => {
                emit_critical_marker(BYE);
                return;
            }
            Some(_) => emit_critical_marker(HELP),
            None => {
                // Halt until something happens -- a key, or the timer. Spinning
                // here would burn a core for thirty seconds -- but only when
                // something can wake the machine up again.
                //
                // The check is not a formality. This loop used to halt
                // unconditionally, with a comment asserting that interrupts were
                // on by this point. Moving the question to the end of the boot
                // made that false: in the demo build the end is reached from
                // inside a syscall handler entered through an interrupt gate,
                // so IF is clear, and the first halt stopped the machine for
                // good. The prompt appeared and nothing after it ever did.
                //
                // SAFETY: `hlt` only with IF set, where the PIT's ~100 Hz tick
                // ends the halt within about 10 ms. With IF clear there is
                // nothing to end it, so the loop spins instead and the deadline
                // above still applies.
                unsafe {
                    let flags: u64;
                    core::arch::asm!("pushfq; pop {}", out(reg) flags, options(nomem, nostack));
                    if flags & (1 << 9) != 0 {
                        core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
                    } else {
                        core::hint::spin_loop();
                    }
                }
            }
        }
    }
}

/// One question and, if anything answers, one answer.
///
/// Split out of the setup above so a second request costs a frame on the wire
/// and nothing else: the rings, the mapping and the unmasked line are already
/// there and are reused rather than rebuilt. Rebuilding them per request would
/// take nine physical frames each time and never give them back.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn ask_once(
    ring: &crate::drivers::e1000::TxRing,
    rx: Option<&crate::drivers::e1000::RxRing>,
    mac: &[u8; 6],
) {
    use crate::drivers::e1000;
    static SEND_FAIL: &[u8] = b"[ZPL-E1000] tx not confirmed by the card FAIL\n";

    let frame = e1000::build_arp_request(mac);
    // SAFETY: `ring` came from `tx_init` and the window is still mapped.
    let polls = match unsafe { e1000::tx_send(ring, &frame) } {
        Ok(p) => p,
        Err(_) => {
            emit_critical_marker(SEND_FAIL);
            return;
        }
    };

    with_irq_masked(|| {
        let mut line = MarkerLine::new();
        line.push(b"[ZPL-E1000] tx arp who-has 10.0.2.2 len=");
        line.push_dec(frame.len() as u64);
        line.push(b" polls=");
        line.push_dec(u64::from(polls));
        line.push(b" confirmed OK\n");
        emit_critical_marker(line.as_slice());
    });

    // Both boot paths wait, and both get an answer. The Limine path did not: it
    // halted on the first turn of the loop and never came back to the timeout
    // check -- measured, with the loop counter and the clock, and fixed by making
    // `interrupts::timer_has_fired` report an observation instead of a setting.
    if let Some(rx) = rx {
        report_arp_reply(rx);
    }
}

/// Map the e1000's registers and read its MAC address off the card.
///
/// The first time this kernel reads a device's own registers rather than the
/// bus's description of one: everything before this came through PCI
/// configuration space, which needs no mapping. Here the card's memory BAR is
/// mapped uncached and two registers are read from it.
///
/// Still reads only. Nothing is written to the device.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn run_e1000_mac_read(scan: &crate::drivers::pci::PciScan) -> Option<[u8; 6]> {
    use crate::drivers::e1000;

    static NO_CARD: &[u8] = b"[ZPL-E1000] no 8086:100e on the bus\n";
    static BAR_BAD: &[u8] = b"[ZPL-E1000] BAR unusable, mac not read FAIL\n";

    let Some(card) = scan.devices.iter().flatten().find(|d| e1000::is_e1000(d)) else {
        emit_critical_marker(NO_CARD);
        return None;
    };

    // SAFETY: `card` came from the enumeration above, so its BAR describes that
    // card's register block. The virtual page the driver maps is its own and is
    // used by nothing else -- see the constant's comment in `drivers/e1000.rs`.
    let mac = match unsafe { e1000::read_mac(card) } {
        Ok(mac) => mac,
        Err(_) => {
            emit_critical_marker(BAR_BAD);
            return None;
        }
    };
    // SAFETY: `read_mac` returned Ok, so the register window is mapped.
    let status = unsafe { e1000::read_status() };

    with_irq_masked(|| {
        let mut line = MarkerLine::new();
        line.push(b"[ZPL-E1000] ");
        push_hex2(&mut line, card.bus);
        line.push(b":");
        push_hex2(&mut line, card.device);
        line.push(b".");
        line.push_dec(u64::from(card.function));
        line.push(b" mac=");
        for (i, byte) in mac.bytes.iter().enumerate() {
            if i > 0 {
                line.push(b":");
            }
            push_hex2(&mut line, *byte);
        }
        line.push(if mac.valid { b" valid" } else { b" INVALID" });
        line.push(if e1000::status_link_up(status) {
            b" link=up"
        } else {
            b" link=down"
        });
        // The prefix is what makes this checkable rather than merely plausible:
        // a wrong read that happened to look like an address would not have it.
        line.push(if mac.looks_like_qemu() {
            b" qemu-prefix OK\n"
        } else {
            b" OK\n"
        });
        emit_critical_marker(line.as_slice());
    });

    Some(mac.bytes)
}

/// Walk the PCI tree and say what is on it, one marker per device.
///
/// Reads only: nothing here programs a base address register, enables bus
/// mastering or writes to a device. What it produces is the first output of this
/// kernel that differs from machine to machine because the machine differs --
/// every field printed was read from real configuration space.
///
/// Placed before the virtio-net probe, which asks the same bus one narrow
/// question, so the log reads as "here is everything, and here is the one thing
/// we were looking for".
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn run_pci_enumeration() {
    use crate::drivers::pci;

    let scan = pci::scan();

    with_irq_masked(|| {
        for slot in scan.devices.iter().take(scan.total.min(scan.devices.len())) {
            let Some(dev) = slot else { continue };
            let mut line = MarkerLine::new();
            line.push(b"[ZPL-PCI] ");
            push_hex2(&mut line, dev.bus);
            line.push(b":");
            push_hex2(&mut line, dev.device);
            line.push(b".");
            line.push_dec(u64::from(dev.function));
            line.push(b" vid=");
            push_hex4(&mut line, dev.vendor);
            line.push(b" did=");
            push_hex4(&mut line, dev.device_id);
            line.push(b" class=");
            push_hex2(&mut line, dev.class);
            line.push(b":");
            push_hex2(&mut line, dev.subclass);
            line.push(b" ");
            line.push(pci::class_name(dev.class, dev.subclass, dev.prog_if));
            if dev.bar0_raw != 0 && dev.bar0_raw != 0xFFFF_FFFF {
                line.push(b" bar0=0x");
                push_hex8(&mut line, dev.bar0_raw);
            }
            if dev.interrupt_line != 0xFF {
                line.push(b" irq=");
                line.push_dec(u64::from(dev.interrupt_line));
            }
            line.push(b"\n");
            emit_critical_marker(line.as_slice());
        }

        let mut summary = MarkerLine::new();
        summary.push(b"[ZPL-PCI] enumerate buses=");
        summary.push_dec(scan.buses_walked as u64);
        summary.push(b" devices=");
        summary.push_dec(scan.total as u64);
        if scan.total > scan.devices.len() {
            summary.push(b" reported=");
            summary.push_dec(scan.devices.len() as u64);
        }
        if scan.truncated {
            summary.push(b" BUS-QUEUE-FULL");
        }
        summary.push(b" OK\n");
        emit_critical_marker(summary.as_slice());
    });

    // The card is already in the scan; walking the bus again to find it
    // would be asking the same hardware the same question twice.
    // Reading the card's address is harmless on any boot path: it maps the
    // register window and reads. Setting the card up is not, and is gated.
    let Some(mac) = run_e1000_mac_read(&scan) else {
        return;
    };

    // Both boot paths. This used to be `qemu_boot` only: on the Limine path the
    // driver hung the boot, and rather than guess why the kernel refused to touch
    // the card there at all. An earlier note here guessed at the interrupt path;
    // that guess was wrong.
    //
    // Measured since, with markers: the last one to print sat before the driver
    // zeroed its descriptor ring, and the one after it never did. The driver was
    // writing to freshly allocated frames through their physical addresses, which
    // is a usable virtual address only where low memory is identity-mapped. The
    // ELF loader had the same bug, found the same way, an hour earlier.
    run_e1000_arm(&scan, &mac);
}

/// Two lower-case hex digits.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn push_hex2(line: &mut MarkerLine, value: u8) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    line.push(&[DIGITS[(value >> 4) as usize], DIGITS[(value & 0xF) as usize]]);
}

/// Eight lower-case hex digits.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn push_hex8(line: &mut MarkerLine, value: u32) {
    push_hex4(line, (value >> 16) as u16);
    push_hex4(line, (value & 0xFFFF) as u16);
}

/// Four lower-case hex digits.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn push_hex4(line: &mut MarkerLine, value: u16) {
    push_hex2(line, (value >> 8) as u8);
    push_hex2(line, (value & 0xFF) as u8);
}

/// PCI config-space scan for `virtio-net-pci` (partial).  Does not
/// program BARs or virtqueues — see `drivers::virtio_net`.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn run_virtio_net_probe() {
    static NO_DEV: &[u8] = b"[ZPL-VNET] probe bus=0 no virtio-net-pci\n";

    with_irq_masked(|| {
        match crate::drivers::virtio_net::scan_virtio_net_pci() {
            None => {
                emit_critical_marker(NO_DEV);
            }
            Some(info) => {
                for byte in b"[ZPL-VNET] probe slot=".iter() {
                    boot_probe_byte(*byte);
                }
                emit_u32_dec(u32::from(info.dev));
                for byte in b" vid=0x".iter() {
                    boot_probe_byte(*byte);
                }
                emit_u16_hex(info.vendor);
                for byte in b" did=0x".iter() {
                    boot_probe_byte(*byte);
                }
                emit_u16_hex(info.device);
                for byte in b" bar0=0x".iter() {
                    boot_probe_byte(*byte);
                }
                emit_u32_hex_full(info.bar0_raw);
                match crate::drivers::virtio_net::probe_legacy_io(info.bar0_raw) {
                    crate::drivers::virtio_net::VirtioIoProbe::LegacyStatus { port_base, status } => {
                        for byte in b" io=0x".iter() {
                            boot_probe_byte(*byte);
                        }
                        emit_u16_hex(port_base);
                        for byte in b" st=0x".iter() {
                            boot_probe_byte(*byte);
                        }
                        emit_u8_hex(status);
                    }
                    crate::drivers::virtio_net::VirtioIoProbe::SkippedMmioOrUnset => {
                        for byte in b" io=skip".iter() {
                            boot_probe_byte(*byte);
                        }
                    }
                }
                for byte in b" OK\n".iter() {
                    boot_probe_byte(*byte);
                }
            }
        }
    });
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn emit_nibbles(value: u32, nibble_count: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for i in (0..nibble_count).rev() {
        let idx = ((value >> (i * 4)) & 0xF) as usize;
        boot_probe_byte(HEX[idx]);
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn emit_u16_hex(v: u16) {
    emit_nibbles(u32::from(v), 4);
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn emit_u8_hex(v: u8) {
    emit_nibbles(u32::from(v), 2);
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn emit_u32_hex_full(v: u32) {
    emit_nibbles(v, 8);
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn run_runqueue_self_check() {
    static MARKER_OK: &[u8] = b"[ZPL-RUNQ] selfcheck N=5 ticks=10 winner=task0_wins=10 OK\n";
    static MARKER_FAIL: &[u8] = b"[ZPL-RUNQ] selfcheck FAIL\n";

    unsafe {
        crate::sched::runqueue::init();
    }
    match crate::sched::runqueue::self_check() {
        Ok(_) => {
            emit_critical_marker(MARKER_OK);
        }
        Err(_) => {
            emit_critical_marker(MARKER_FAIL);
        }
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn run_matrix_ops_self_check() {
    static MARKER_OK: &[u8] = b"[ZPL-MATRIX] selfcheck 8ops naive==simd 64x64 OK\n";
    static MARKER_FAIL: &[u8] = b"[ZPL-MATRIX] selfcheck FAIL\n";

    match crate::matrix_ops::self_check() {
        Ok(_) => {
            emit_critical_marker(MARKER_OK);
        }
        Err(_) => {
            emit_critical_marker(MARKER_FAIL);
        }
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn run_shm_self_check() {
    // The SHM self-check writes a "clean" payload then a "hostile" one and
    // asserts ALLOW then BLOCK. It used to be skipped in the public build,
    // because the stub there refused every write and the first assertion
    // would have fired spuriously. The demo policy allows the clean write
    // and refuses the hostile one -- which is the point of it -- so the
    // check now runs in both builds.
    static MARKER_OK: &[u8] = b"[ZPL-SHM] selfcheck allow_clean+block_hostile OK\n";
    static MARKER_FAIL: &[u8] = b"[ZPL-SHM] selfcheck FAIL\n";

    unsafe {
        crate::ipc::shm::init();
    }

    match crate::ipc::shm::self_check() {
        Ok(_) => {
            emit_critical_marker(MARKER_OK);
        }
        Err(_) => {
            emit_critical_marker(MARKER_FAIL);
        }
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn run_ipc_self_check() {
    static MARKER_OK: &[u8] = b"[ZPL-IPC] selfcheck pipe+send+recv ipc-roundtrip OK\n";
    static MARKER_FAIL: &[u8] = b"[ZPL-IPC] selfcheck FAIL\n";

    unsafe {
        crate::ipc::ipc::init();
    }
    match crate::ipc::ipc::self_check() {
        Ok(_) => {
            emit_critical_marker(MARKER_OK);
        }
        Err(_) => {
            emit_critical_marker(MARKER_FAIL);
        }
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn run_process_self_check() {
    static MARKER_OK: &[u8] = b"[ZPL-PROC] selfcheck spawn=3 wait=3 exit_codes_sum=43 live=0 OK\n";
    static MARKER_FAIL: &[u8] = b"[ZPL-PROC] selfcheck FAIL\n";

    unsafe {
        crate::sched::process::init();
    }
    match crate::sched::process::self_check() {
        Ok(_) => {
            emit_critical_marker(MARKER_OK);
        }
        Err(_) => {
            emit_critical_marker(MARKER_FAIL);
        }
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn run_ramfs_self_check() {
    static MARKER_OK: &[u8] = b"[ZPL-RAMFS] selfcheck create+write+close+reopen+read OK\n";
    static MARKER_FAIL: &[u8] = b"[ZPL-RAMFS] selfcheck FAIL\n";

    unsafe {
        crate::fs::ramfs::init();
    }
    match crate::fs::ramfs::self_check() {
        Ok(_) => {
            emit_critical_marker(MARKER_OK);
        }
        Err(_) => {
            emit_critical_marker(MARKER_FAIL);
        }
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn run_stack_guard_demo() {
    static MARKER_INSTALL: &[u8] = b"[ZPL-STACKGUARD] install body=8KiB guard=0x40004000 ok\n";
    static MARKER_SMOKE: &[u8] = b"[ZPL-STACKGUARD] body_write_read ok\n";
    static MARKER_GUARD_HIT: &[u8] = b"[ZPL-STACKGUARD] guard_hit virt=0x40004000 cleanly handled\n";
    static MARKER_GUARD_MISS: &[u8] = b"[ZPL-STACKGUARD] guard_hit NOT observed\n";
    static MARKER_FAIL: &[u8] = b"[ZPL-STACKGUARD] install FAIL\n";

    unsafe {
        core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
    }

    if unsafe { crate::mm::stack_guard::install() }.is_err() {
        emit_critical_marker(MARKER_FAIL);
        unsafe {
            core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
        }
        return;
    }
    emit_critical_marker(MARKER_INSTALL);
    if crate::mm::stack_guard::smoke_write() {
        emit_critical_marker(MARKER_SMOKE);
    }

    let before = crate::mm::stack_guard::page_fault_count();
    crate::mm::stack_guard::trigger_overflow();
    let after = crate::mm::stack_guard::page_fault_count();
    if after > before {
        emit_critical_marker(MARKER_GUARD_HIT);
    } else {
        emit_critical_marker(MARKER_GUARD_MISS);
    }

    unsafe {
        core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn run_paging_pf_demo() {
    static MARKER_PF: &[u8] = b"[ZPL-PAGING] page_fault virt=0x40000000 reread ok\n";
    static MARKER_PF_FAIL: &[u8] = b"[ZPL-PAGING] page_fault NOT triggered\n";

    // Page-fault is a CPU exception that fires regardless of IF, so we can
    // disable maskable interrupts here to keep the timer ISR from
    // interleaving with the marker bytes on COM1. We re-enable on exit.
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
    }

    let _ = crate::mm::paging::trigger_pf_re_read();
    if crate::mm::paging::PAGE_FAULT_COUNT.load(core::sync::atomic::Ordering::SeqCst) >= 1 {
        emit_critical_marker(MARKER_PF);
    } else {
        emit_critical_marker(MARKER_PF_FAIL);
    }

    unsafe {
        core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn run_smp_probe() {
    static MARKER_FAIL: &[u8] = b"[ZPL-SMP] probe FAIL no basic cpuid\n";

    match crate::smp::self_check() {
        Ok(probe) => {
            // Format: [ZPL-SMP max_leaf=N max_logical=M apic_id=X OK]\n
            //
            // Built into one buffer, alloc-free, so the whole line goes out as a
            // single marker: byte-at-a-time it reached COM1 only and never the
            // screen.
            let mut line = MarkerLine::new();
            line.push(b"[ZPL-SMP max_leaf=");
            line.push_dec(u64::from(probe.max_leaf));
            line.push(b" max_logical=");
            line.push_dec(u64::from(probe.max_logical_cpus));
            line.push(b" apic_id=");
            line.push_dec(u64::from(probe.apic_id));
            line.push(b" OK]\n");
            emit_critical_marker(line.as_slice());
        }
        Err(_) => {
            emit_critical_marker(MARKER_FAIL);
        }
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn emit_u32_dec(mut value: u32) {
    if value == 0 {
        boot_probe_byte(b'0');
        return;
    }
    let mut buf = [0u8; 10];
    let mut idx = 0;
    while value > 0 {
        buf[idx] = b'0' + (value % 10) as u8;
        value /= 10;
        idx += 1;
    }
    while idx > 0 {
        idx -= 1;
        boot_probe_byte(buf[idx]);
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn run_audit_chain_self_check() {
    static MARKER_OK: &[u8] =
        b"[ZPL-AUDIT] selfcheck append=5 verify=ok tamper_detected OK\n";
    static MARKER_FAIL: &[u8] = b"[ZPL-AUDIT] selfcheck FAIL\n";

    unsafe {
        crate::audit::audit_chain::init();
    }
    match crate::audit::audit_chain::self_check() {
        Ok(_) => emit_critical_marker(MARKER_OK),
        Err(_) => emit_critical_marker(MARKER_FAIL),
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn run_heap_self_check() {
    static MARKER_OK: &[u8] = b"[ZPL-HEAP] selfcheck box+vec100 OK\n";
    static MARKER_FAIL: &[u8] = b"[ZPL-HEAP] selfcheck FAIL\n";

    unsafe {
        crate::mm::heap::init();
    }
    match crate::mm::heap::self_check() {
        Ok(_) => {
            emit_critical_marker(MARKER_OK);
        }
        Err(_) => {
            emit_critical_marker(MARKER_FAIL);
        }
    }
}

fn run_frame_alloc_self_check() {
    // Measured, not asserted. This line used to be a fixed string: it printed the same
    // two numbers on every machine, whatever the firmware had actually left free, and
    // went on printing them after the allocator learned to read the memory map. A log
    // line that cannot be wrong cannot be evidence either.
    //
    // `cap` is now what the allocator may hand out here -- free plus taken -- which on
    // a machine whose firmware kept memory for itself is smaller than the bitmap.
    static MARKER_BURST_OK: &[u8] = b"[ZPL-FRAME] burst alloc=1000 free=1000 leak=0\n";
    static MARKER_STRESS_OK: &[u8] = b"[ZPL-FRAME] stress=1000000 leak=0\n";
    static MARKER_FAIL: &[u8] = b"[ZPL-FRAME] selfcheck FAIL\n";

    crate::mm::frame_alloc::init(crate::mm::frame_alloc::BASE_PHYS_DEFAULT);
    {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut buf = [0u8; 64];
        let mut n = 0;
        for b in b"[ZPL-FRAME] init base=0x" {
            buf[n] = *b;
            n += 1;
        }
        let base = crate::mm::frame_alloc::BASE_PHYS_DEFAULT;
        let mut shift = 28;
        loop {
            buf[n] = HEX[((base >> shift) & 0xF) as usize];
            n += 1;
            if shift == 0 {
                break;
            }
            shift -= 4;
        }
        for b in b" cap=" {
            buf[n] = *b;
            n += 1;
        }
        let cap = crate::mm::frame_alloc::capacity_frames();
        if cap == 0 {
            buf[n] = b'0';
            n += 1;
        } else {
            let mut digits = [0u8; 20];
            let mut d = 0;
            let mut v = cap;
            while v > 0 {
                digits[d] = b'0' + (v % 10) as u8;
                d += 1;
                v /= 10;
            }
            while d > 0 {
                d -= 1;
                buf[n] = digits[d];
                n += 1;
            }
        }
        buf[n] = b'\n';
        n += 1;
        emit_critical_marker(&buf[..n]);
    }
    match crate::mm::frame_alloc::self_check() {
        Ok(_) => {
            emit_critical_marker(MARKER_BURST_OK);
            emit_critical_marker(MARKER_STRESS_OK);
        }
        Err(_) => {
            emit_critical_marker(MARKER_FAIL);
        }
    }
}


/// ~500 ms busy-wait using TSC (assumes TSC ≥ 1.5 GHz; slower CPUs wait longer).
#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    feature = "vga_visual_hold",
))]
fn delay_rdtsc_approx_500ms() {
    const MIN_TSC_HZ: u64 = 1_500_000_000;
    let ticks = MIN_TSC_HZ / 2;
    let t0 = unsafe { core::arch::x86_64::_rdtsc() };
    while unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(t0) < ticks {
        // The prompt is read here, inside the wait, not only between waits.
        //
        // The i8042 output buffer holds exactly one byte. A loop that polled once per
        // half-second would therefore keep one character in every half-second of typing
        // and lose the rest -- and a keyboard that drops four keys in five is not a
        // keyboard. Measured before this existed: eleven characters sent to QEMU came
        // back one per nine ticks, because that was the rate the emulator injected them
        // at, not the rate the kernel could take them.
        #[cfg(feature = "shell")]
        crate::ui::shell::kernel::poll();
        core::hint::spin_loop();
    }
}

#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    feature = "vga_visual_hold",
))]
fn push_decimal_u32_into(mut n: u32, buf: &mut [u8], start: usize) -> usize {
    let mut pos = start;
    if n == 0 {
        buf[pos] = b'0';
        return pos + 1;
    }
    let begin = pos;
    while n > 0 {
        buf[pos] = b'0' + (n % 10) as u8;
        pos += 1;
        n /= 10;
    }
    let mut lo = begin;
    let mut hi = pos - 1;
    while lo < hi {
        buf.swap(lo, hi);
        lo += 1;
        hi -= 1;
    }
    pos
}

/// Draw the v0.4 summary block into the bottom three framebuffer rows.
///
/// Drawn only on screen, once per tick: it is a live readout, not an event, so it stays
/// out of the serial log and therefore out of the determinism hash. The hash it shows is
/// recomputable from the log with the same filter the determinism method uses.
#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    feature = "v04_menu",
    not(feature = "qemu_boot"),
    // The panel draws the same numbers as one of its own rows; two blocks wanting the
    // bottom of the screen is what kept the panel off the ISO in the first place.
    not(feature = "gate_panel_fb"),
))]
fn draw_v04_summary(tick: u32) {
    use crate::drivers::fbcon;
    use crate::ui::v04_status;

    if !fbcon::is_fb_ready() {
        return;
    }
    let mut buf = [0u8; 120];

    // Row 2 from the bottom: the tick counter.
    fbcon::jump_cursor_row_from_bottom(2);
    fbcon::set_color(fbcon::FB_WHITE);
    let mut end = 0;
    end = push_ascii_into(b"tick=", &mut buf, end);
    end = push_decimal_u32_into(tick, &mut buf, end);
    fbcon::write_bytes(&buf[..end]);

    // Row 1: the most recent decision, if any.
    fbcon::jump_cursor_row_from_bottom(1);
    let mut end = push_ascii_into(b"last decision: ", &mut buf, 0);
    match v04_status::last_decision() {
        Some((ain, action)) => {
            let (label, colour): (&[u8], u32) = match action {
                b'B' => (b"BLOCK", fbcon::FB_RED),
                b'D' => (b"DEGRADE", fbcon::FB_YELLOW),
                _ => (b"ALLOW", fbcon::FB_GREEN),
            };
            end = push_ascii_into(b"ain=", &mut buf, end);
            end = push_decimal_u32_into(ain, &mut buf, end);
            end = push_ascii_into(b" ", &mut buf, end);
            end = push_ascii_into(label, &mut buf, end);
            fbcon::set_color(colour);
        }
        None => {
            end = push_ascii_into(b"(none yet)", &mut buf, end);
            fbcon::set_color(fbcon::FB_WHITE);
        }
    }
    fbcon::write_bytes(&buf[..end]);

    // Row 0 (bottom): the one-line summary, ASCII only.
    fbcon::jump_cursor_row_from_bottom(0);
    fbcon::set_color(fbcon::FB_WHITE);
    let mut end = push_version_banner_into(&mut buf);
    end = push_decimal_u32_into(v04_status::selfchecks_ok(), &mut buf, end);
    end = push_ascii_into(b" self-checks OK, ", &mut buf, end);
    end = push_decimal_u32_into(v04_status::decisions(), &mut buf, end);
    end = push_ascii_into(b" decisions, hash ", &mut buf, end);
    end = push_hex_u64_into(v04_status::hash(), &mut buf, end);
    fbcon::write_bytes(&buf[..end]);
}

/// Write `ZPL kernel <version> - ` at the start of `buf`, and say how far it got.
///
/// Deliberately not gated on a target or a feature, and deliberately not built
/// on `push_ascii_into`: it has to be callable from a host test, because the
/// point of it is the test below. The version comes from [`crate::KERNEL_VERSION`]
/// and from nowhere else -- a literal written here again is the exact bug this
/// replaced, and the test is what would catch it coming back.
// Nothing on the bare-metal side calls this any more: the `qemu_boot` build never
// drew that summary, and on the Limine build the panel's own bottom row took it
// over. It stays anyway, and stays ungated, because the test below is the whole
// point of it -- and a test that only compiles in a configuration nobody ships is
// not much of a test. The panel reads the same `KERNEL_VERSION`, so what the test
// guards is still the property that matters: one version, written in one place.
#[cfg_attr(
    any(feature = "qemu_boot", feature = "gate_panel_fb"),
    allow(dead_code)
)]
pub(crate) fn push_version_banner_into(buf: &mut [u8]) -> usize {
    let mut pos = 0usize;
    for part in [b"ZPL kernel ".as_slice(), crate::KERNEL_VERSION.as_bytes(), b" - ".as_slice()] {
        for &byte in part {
            if pos >= buf.len() {
                return pos;
            }
            buf[pos] = byte;
            pos += 1;
        }
    }
    pos
}

#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    feature = "v04_menu",
    not(feature = "qemu_boot"),
    not(feature = "gate_panel_fb"),
))]
fn push_ascii_into(src: &[u8], buf: &mut [u8], start: usize) -> usize {
    let mut pos = start;
    for &b in src {
        if pos >= buf.len() {
            break;
        }
        buf[pos] = b;
        pos += 1;
    }
    pos
}

#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    feature = "v04_menu",
    not(feature = "qemu_boot"),
    // The panel draws the same numbers as one of its own rows; two blocks wanting the
    // bottom of the screen is what kept the panel off the ISO in the first place.
    not(feature = "gate_panel_fb"),
))]
fn push_hex_u64_into(v: u64, buf: &mut [u8], start: usize) -> usize {
    const DIGIT: &[u8; 16] = b"0123456789abcdef";
    let mut pos = start;
    for n in (0..16).rev() {
        if pos >= buf.len() {
            break;
        }
        buf[pos] = DIGIT[((v >> (n * 4)) & 0xf) as usize];
        pos += 1;
    }
    pos
}

/// Halt path that keeps writing VGA/COM1 so HDMI text mode stays visibly alive on some firmware.
#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    feature = "vga_visual_hold",
))]
fn run_visual_alive_loop() -> ! {
    use core::sync::atomic::{AtomicU32, Ordering};

    static ALIVE_TICK: AtomicU32 = AtomicU32::new(0);
    // The menu banner goes up once, after the self-check output, before the ticker
    // starts overwriting the bottom row.
    #[cfg(feature = "v04_menu")]
    crate::ui::v04_menu::show_banner();
    // The prompt goes up after the menu banner, so the last thing on the screen before
    // the ticker starts is something to type at.
    #[cfg(feature = "shell")]
    {
        crate::ui::shell::kernel::seed_programs();
        crate::ui::shell::kernel::banner();
    }
    loop {
        // v0.4: keys are read here, in the halt path, so interrupt routing and the
        // scheduler are untouched. A key press only ever adds `[ZPL-V04 …]` lines,
        // which the determinism hash excludes.
        // With the shell on, the keyboard belongs to the prompt: reading the same
        // controller from two places would mean each reader swallowing half the other's
        // scancodes, so the menu's three-key reader stands down.
        #[cfg(all(feature = "v04_menu", not(feature = "shell")))]
        if let Some(key) = crate::drivers::kbd::poll_menu_key() {
            crate::ui::v04_menu::handle(key);
        }
        #[cfg(feature = "shell")]
        crate::ui::shell::kernel::poll();
        let n = ALIVE_TICK.fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "shell")]
        crate::ui::shell::note_tick(n);
        let mut buf = [0u8; 56];
        const PREFIX: &[u8] = b"[ZPL-ALIVE] tick=";
        buf[..PREFIX.len()].copy_from_slice(PREFIX);
        let mut end = push_decimal_u32_into(n, &mut buf, PREFIX.len());
        buf[end] = b'\n';
        end += 1;
        // With the panel on, the tick is one field of the panel's bottom row, so the
        // marker goes to COM1 and stops there. Mirrored to the screen as well it would
        // land on the bottom text row twice a second and paint over whatever the prompt
        // has written there -- which is the whole reason the summary moved into the
        // panel instead of sitting under it.
        #[cfg(feature = "gate_panel_fb")]
        {
            crate::drivers::console::boot_probe_slice_no_vga(&buf[..end]);
            crate::ui::gate_panel::note_tick(n);
        }
        #[cfg(not(feature = "gate_panel_fb"))]
        {
            crate::drivers::vga::jump_cursor_last_row_start();
            // Keep the tick pinned to the bottom row on the framebuffer too, instead of
            // letting it scroll the self-check output off the screen. The cursor is
            // parked first and put back afterwards, so ordinary lines (the v0.4 menu
            // results) keep flowing under the banner instead of landing on the ticker
            // row and being overwritten half a second later.
            #[cfg(feature = "fb_crit_mirror")]
            crate::drivers::fbcon::save_cursor();
            // With the v0.4 summary present the bottom three rows belong to it, so the
            // ticker line parks one row above them.
            #[cfg(all(feature = "fb_crit_mirror", feature = "v04_menu"))]
            crate::drivers::fbcon::jump_cursor_row_from_bottom(3);
            #[cfg(all(feature = "fb_crit_mirror", not(feature = "v04_menu")))]
            crate::drivers::fbcon::jump_cursor_last_row_start();
            with_irq_masked(|| {
                emit_critical_marker(&buf[..end]);
            });
            #[cfg(all(
                target_os = "none",
                target_arch = "x86_64",
                feature = "v04_menu",
                not(feature = "qemu_boot"),
            ))]
            draw_v04_summary(n);
            #[cfg(feature = "fb_crit_mirror")]
            crate::drivers::fbcon::restore_cursor();
        }
        delay_rdtsc_approx_500ms();
    }
}

#[cfg(test)]
mod marker_line_tests {
    use crate::drivers::console::MarkerLine;

    #[test]
    fn pieces_join_into_one_line() {
        let mut line = MarkerLine::new();
        line.push(b"[ZPL-SMP max_leaf=");
        line.push_dec(13);
        line.push(b" apic_id=");
        line.push_dec(0);
        line.push(b" OK]\n");
        assert_eq!(line.as_slice(), b"[ZPL-SMP max_leaf=13 apic_id=0 OK]\n");
    }

    #[test]
    fn zero_prints_as_one_digit_not_empty() {
        let mut line = MarkerLine::new();
        line.push_dec(0);
        assert_eq!(line.as_slice(), b"0");
    }

    #[test]
    fn hex64_is_sixteen_zero_padded_lower_case_digits() {
        let mut line = MarkerLine::new();
        line.push_hex64(0x11d000);
        assert_eq!(line.as_slice(), b"000000000011d000");
    }

    #[test]
    fn overflow_truncates_instead_of_panicking() {
        // A marker longer than the buffer must not fault in early boot, where
        // there is no unwinder and no way to report the panic.
        let mut line = MarkerLine::new();
        line.push(&[b'x'; 200]);
        assert_eq!(line.as_slice().len(), MarkerLine::CAPACITY);
        line.push(b"more");
        assert_eq!(line.as_slice().len(), MarkerLine::CAPACITY);
    }

    #[test]
    fn the_summary_line_says_the_same_version_as_the_build_id_marker() {
        // These were two separate string literals once, and a version bump moved
        // one of them. A machine booted from a stick then showed
        // `version=v0.5.0` at the top of the screen and `ZPL kernel v0.4` at the
        // bottom, both true of the same binary. Found from a photograph of real
        // hardware, which is a slow way to find it.
        //
        // Both now read `KERNEL_VERSION`. This fails if a literal is written
        // back into the banner.
        let mut buf = [0u8; 64];
        let n = super::push_version_banner_into(&mut buf);
        let banner = core::str::from_utf8(&buf[..n]).expect("ascii");

        assert!(
            banner.contains(crate::KERNEL_VERSION),
            "the summary banner is {banner:?}, which does not contain {:?}",
            crate::KERNEL_VERSION
        );
        // Built the same way rather than formatted, because this crate is
        // `no_std` and `format!` is not there.
        let mut want = [0u8; 64];
        let mut w = 0usize;
        for part in [b"ZPL kernel ".as_slice(), crate::KERNEL_VERSION.as_bytes(), b" - ".as_slice()] {
            want[w..w + part.len()].copy_from_slice(part);
            w += part.len();
        }
        assert_eq!(banner.as_bytes(), &want[..w]);
    }

    #[test]
    fn the_version_looks_like_a_version() {
        // Cheap, and it is the shape the marker parser and CITATION.cff assume.
        let v = crate::KERNEL_VERSION;
        assert!(v.starts_with('v'), "{v:?} should start with v");
        assert_eq!(v[1..].split('.').count(), 3, "{v:?} should be v<major>.<minor>.<patch>");
        assert!(
            v[1..].split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())),
            "{v:?} has a part that is not a number"
        );
    }

    #[test]
    fn a_truncated_marker_still_ends_in_a_newline() {
        // Otherwise it runs into the next marker and the log shows one line
        // that neither of them wrote. That happened, in the receive marker,
        // which is why this test exists.
        let mut line = MarkerLine::new();
        line.push(&[b'x'; 300]);
        line.push(b" tail that will not fit\n");
        assert!(
            line.as_slice().ends_with(b"\n"),
            "a marker that overflowed did not end"
        );
    }

    #[test]
    fn the_longest_real_marker_still_fits() {
        // The SMP probe with every field at its widest, which is the line that
        // sized the buffer.
        let mut line = MarkerLine::new();
        line.push(b"[ZPL-SMP max_leaf=");
        line.push_dec(u64::from(u32::MAX));
        line.push(b" max_logical=");
        line.push_dec(u64::from(u32::MAX));
        line.push(b" apic_id=");
        line.push_dec(u64::from(u32::MAX));
        line.push(b" OK]\n");
        assert!(
            line.as_slice().len() < MarkerLine::CAPACITY,
            "len {}",
            line.as_slice().len()
        );
        assert!(line.as_slice().ends_with(b" OK]\n"));
    }
}
