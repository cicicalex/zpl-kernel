//! CPUID-based topology probe (partial scope).
//!
//! v0.1 reads three CPUID leaves and reports them on COM1 so the
//! kernel can confirm it sees the SMP topology QEMU advertised. AP
//! bring-up (INIT-SIPI-SIPI sequence + per-CPU stack + APIC timer
//! per core) is the substantial follow-up that completes `#36`; this
//! probe is the scaffolding the bring-up hooks into.

#![cfg(target_arch = "x86_64")]

use core::arch::x86_64::__cpuid;

#[derive(Debug, Clone, Copy)]
pub struct CpuidProbe {
    /// Highest CPUID basic leaf supported by the BSP.
    pub max_leaf: u32,
    /// `CPUID.01H:EBX[23:16]` -- maximum number of addressable IDs for
    /// logical processors in this physical package as advertised by
    /// the firmware. Not the runtime online count.
    pub max_logical_cpus: u8,
    /// `CPUID.01H:EBX[31:24]` -- BSP local APIC ID.
    pub apic_id: u8,
    /// CPUID.00H vendor string, EBX:EDX:ECX in that order.
    pub vendor: [u8; 12],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfCheckErr {
    /// CPUID returned a max_leaf below 1, meaning we cannot read the
    /// feature flags we depend on.
    NoBasicCpuid,
    /// CPUID.00H's vendor string is not printable ASCII. Every real
    /// x86 vendor tag is ("GenuineIntel", "AuthenticAMD", and the
    /// hypervisor tags alike), so this means EBX did not survive the
    /// call -- exactly the failure mode described below.
    VendorNotAscii,
}

/// Read one CPUID leaf.
///
/// **Do not hand-roll this with `asm!`.** The obvious version --
/// `push rbx` / `cpuid` / `mov {b:e}, ebx` / `pop rbx`, binding the
/// output with `lateout(reg)` -- looks correct and is not: the register
/// allocator is free to choose `rbx` itself for that output, and then
/// the copy is `mov ebx, ebx` and the following `pop rbx` overwrites
/// the result with the saved value. That is what this kernel shipped
/// until 26 September 2026; `apic_id` and `max_logical_cpus` were
/// reading a stale caller register (a kernel pointer) rather than
/// hardware. `__cpuid` gets the constraints right.
#[inline]
// `__cpuid` is unsafe on the host target and safe on the kernel target
// spec, so the block is required in one build and redundant in the other.
#[allow(unused_unsafe)]
fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    // SAFETY: CPUID is unprivileged and has no side effects. Leaf 0 is
    // architecturally guaranteed; callers check `max_leaf` before
    // trusting any higher leaf.
    let r = unsafe { __cpuid(leaf) };
    (r.eax, r.ebx, r.ecx, r.edx)
}

pub fn probe() -> CpuidProbe {
    let (max_leaf, v_ebx, v_ecx, v_edx) = cpuid(0);
    let (_, ebx, _, _) = cpuid(1);
    let max_logical_cpus = ((ebx >> 16) & 0xFF) as u8;
    let apic_id = ((ebx >> 24) & 0xFF) as u8;

    // Vendor string order is EBX, then EDX, then ECX.
    let mut vendor = [0u8; 12];
    for (i, word) in [v_ebx, v_edx, v_ecx].iter().enumerate() {
        vendor[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }

    CpuidProbe {
        max_leaf,
        max_logical_cpus,
        apic_id,
        vendor,
    }
}

pub fn self_check() -> Result<CpuidProbe, SelfCheckErr> {
    let probe = probe();
    if probe.max_leaf < 1 {
        return Err(SelfCheckErr::NoBasicCpuid);
    }
    if !probe.vendor.iter().all(|b| (0x20..0x7f).contains(b)) {
        return Err(SelfCheckErr::VendorNotAscii);
    }
    Ok(probe)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The regression test for the `rbx` clobber.
    ///
    /// With the broken inline asm, CPUID.00H:EBX came back as whatever
    /// the caller happened to leave in `rbx` -- in the kernel, the low
    /// half of a pointer. This asserts the bytes are a printable vendor
    /// tag, which no pointer ever is.
    #[test]
    fn cpuid_leaf_zero_returns_a_printable_vendor_string() {
        let p = probe();
        assert!(
            p.vendor.iter().all(|b| (0x20..0x7f).contains(b)),
            "CPUID.00H vendor bytes are not printable ASCII: {:?} -- EBX did not survive the call",
            p.vendor
        );
    }

    /// Known vendor tags, as a second and stricter check on the same
    /// register path. If this host is neither Intel nor AMD nor a
    /// hypervisor with an ASCII tag, the assertion above still holds
    /// and this one is the informative failure.
    #[test]
    fn vendor_string_is_one_of_the_known_tags() {
        let p = probe();
        let s = core::str::from_utf8(&p.vendor).expect("vendor is ASCII");
        const KNOWN: [&str; 6] = [
            "GenuineIntel",
            "AuthenticAMD",
            "KVMKVMKVM\0\0\0",
            "Microsoft Hv",
            "VMwareVMware",
            "TCGTCGTCGTCG",
        ];
        assert!(KNOWN.contains(&s), "unexpected CPUID vendor tag: {s:?}");
    }

    /// `max_leaf` was always correct -- it comes back in EAX, which the
    /// broken asm did bind properly. Pinned so a future rewrite cannot
    /// break it silently.
    #[test]
    fn max_leaf_is_at_least_one() {
        assert!(probe().max_leaf >= 1);
    }

    #[test]
    fn self_check_passes_on_this_host() {
        assert!(self_check().is_ok());
    }
}
