//! `zpl-asm` v0.1 — minimal x86_64 / ZPL textual assembler.
//!
//! Goal: take a `.zpla` source file and produce an
//! ELF64 binary that the kernel ELF loader (`crate::fs::elf`) can boot in
//! ring 3.
//!
//! Scope of v0.1:
//! - Mnemonics: `mov rax, <imm>`, `mov rdi, <imm>`, `mov rsi, <imm>`,
//!   `int <imm>`, ZPL operators `xor/and/or` (32-bit reg-reg form for
//!   parity with the spec).
//! - Directives: `entry <label>`, `org <vaddr>`, `db "string"`,
//!   `db <byte>`.
//! - Labels: `name:` on its own line; references resolved at codegen.
//! - Output: ELF64 with one PT_LOAD covering [`org`, `org + payload`),
//!   `e_entry` set from the `entry` directive.
//!
//! Out of scope:
//! - Full x86_64 instruction set (RM/MR/SIB encoding).
//! - Multiple sections, shared libs, relocations.
//! - Macros for `NAND` / `NOR` / `XNOR` / `IMP` / `NIMP` / `AIN` / `SCHED`
//!   — recognized as reserved mnemonics today but emit a "not yet
//!   implemented" diagnostic. They will be lowered to AND-NOT / OR-NOT /
//!   etc sequences in a follow-up.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "zpl-asm", about = "ZPL minimal assembler -> ELF64")]
struct Cli {
    /// Path to the `.zpla` source file.
    input: PathBuf,
    /// Output ELF64 path.
    #[arg(long, short = 'o')]
    out: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let src = fs::read_to_string(&cli.input)
        .with_context(|| format!("[E_ASM_001_READ] reading {}", cli.input.display()))?;
    let module = parse(&src)?;
    let bytes = build_elf(&module)?;
    fs::write(&cli.out, &bytes)
        .with_context(|| format!("[E_ASM_002_WRITE] writing {}", cli.out.display()))?;
    println!(
        "zpl-asm: wrote {} bytes ({} instructions, entry=0x{:x})",
        bytes.len(),
        module.instr_count,
        module.entry_addr
    );
    Ok(())
}

#[derive(Debug)]
enum Op {
    MovRaxImm(MovImm),
    MovRdiImm(MovImm),
    MovRsiImm(MovImm),
    IntImm(u8),
    /// 32-bit reg-reg form `xor reg, reg2` etc., kept compact for v0.1.
    /// Only `0x31 /r` (XOR), `0x21 /r` (AND), `0x09 /r` (OR) are emitted.
    BitwiseRR { opcode: u8, reg_dst: u8, reg_src: u8 },
    DbBytes(Vec<u8>),
    LabelDef { name: String },
}

#[derive(Debug)]
enum MovImm {
    Literal(u64),
    Label(String),
}

#[derive(Debug)]
struct Module {
    org: u64,
    entry_label: String,
    entry_addr: u64,
    payload: Vec<u8>,
    /// Total number of instruction or db statements parsed.
    instr_count: u32,
}

const DEFAULT_ORG: u64 = 0x4001_0000;

// Over the limit at 168 lines, and allowed with the debt named rather than split
// quietly. It is two passes in one function: collect statements with their line
// numbers, then resolve labels and emit. Splitting it is the right move and it is
// not done here, because until today this crate had no tests at all and splitting a
// parser nobody is checking is how an assembler starts producing programs that fail
// inside a kernel. The seven tests below are the first half of that; the split is
// the second.
#[allow(clippy::too_many_lines)]
fn parse(src: &str) -> Result<Module> {
    let mut org = DEFAULT_ORG;
    let mut entry_label: Option<String> = None;
    let mut ops: Vec<(usize, Op)> = Vec::new();
    let mut labels: HashMap<String, ()> = HashMap::new();

    // First pass: collect statements with line numbers.
    for (line_no, raw_line) in src.lines().enumerate() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }

        if let Some(label) = line.strip_suffix(':') {
            let name = label.trim().to_string();
            if labels.contains_key(&name) {
                bail!("[E_ASM_010_DUP_LABEL] line {}: duplicate label `{}`", line_no + 1, name);
            }
            labels.insert(name.clone(), ());
            ops.push((line_no, Op::LabelDef { name }));
            continue;
        }

        let mut parts = line.splitn(2, char::is_whitespace);
        let head = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("").trim();
        match head.to_ascii_lowercase().as_str() {
            "entry" => {
                if rest.is_empty() {
                    bail!("[E_ASM_011_ENTRY_ARG] line {}: `entry` needs label", line_no + 1);
                }
                entry_label = Some(rest.to_string());
            }
            "org" => {
                org = parse_int(rest)
                    .with_context(|| format!("[E_ASM_012_ORG] line {}", line_no + 1))?;
            }
            "db" => {
                let bytes = parse_db(rest)
                    .with_context(|| format!("[E_ASM_013_DB] line {}", line_no + 1))?;
                ops.push((line_no, Op::DbBytes(bytes)));
            }
            "mov" => {
                let (dst, src_str) = split2(rest)?;
                match dst.to_ascii_lowercase().as_str() {
                    "rax" => {
                        let imm = parse_mov_imm(src_str, src, &labels)
                            .with_context(|| format!("[E_ASM_014_MOV_RAX] line {}", line_no + 1))?;
                        ops.push((line_no, Op::MovRaxImm(imm)));
                    }
                    "rdi" => {
                        let imm = parse_mov_imm(src_str, src, &labels)
                            .with_context(|| format!("[E_ASM_015_MOV_RDI] line {}", line_no + 1))?;
                        ops.push((line_no, Op::MovRdiImm(imm)));
                    }
                    "rsi" => {
                        let imm = parse_mov_imm(src_str, src, &labels)
                            .with_context(|| format!("[E_ASM_016_MOV_RSI] line {}", line_no + 1))?;
                        ops.push((line_no, Op::MovRsiImm(imm)));
                    }
                    other => {
                        bail!(
                            "[E_ASM_017_MOV_DST] line {}: unsupported mov destination `{}`",
                            line_no + 1,
                            other
                        );
                    }
                }
            }
            "int" => {
                let imm = parse_int(rest)
                    .with_context(|| format!("[E_ASM_020_INT] line {}", line_no + 1))?;
                if imm > 0xFF {
                    bail!("[E_ASM_021_INT_RANGE] line {}: int operand too large", line_no + 1);
                }
                ops.push((line_no, Op::IntImm(imm as u8)));
            }
            "xor" | "and" | "or" => {
                let opcode = match head {
                    "xor" => 0x31u8,
                    "and" => 0x21,
                    "or" => 0x09,
                    _ => unreachable!(),
                };
                let (dst, src_s) = split2(rest)?;
                let dst_reg = parse_reg32(dst)
                    .with_context(|| format!("[E_ASM_022_BITWISE_DST] line {}", line_no + 1))?;
                let src_reg = parse_reg32(src_s)
                    .with_context(|| format!("[E_ASM_023_BITWISE_SRC] line {}", line_no + 1))?;
                ops.push((
                    line_no,
                    Op::BitwiseRR {
                        opcode,
                        reg_dst: dst_reg,
                        reg_src: src_reg,
                    },
                ));
            }
            "nand" | "nor" | "xnor" | "imp" | "nimp" | "ain" | "sched" => {
                bail!(
                    "[E_ASM_024_RESERVED] line {}: mnemonic `{}` is reserved \
                     but not implemented in v0.1",
                    line_no + 1,
                    head
                );
            }
            other => {
                bail!("[E_ASM_030_UNKNOWN] line {}: unknown directive `{}`", line_no + 1, other);
            }
        }
    }

    let entry_label = entry_label
        .ok_or_else(|| anyhow!("[E_ASM_040_NO_ENTRY] missing `entry <label>` directive"))?;

    // Pass 2: layout and resolve labels.
    let mut payload: Vec<u8> = Vec::new();
    let mut label_addrs: HashMap<String, u64> = HashMap::new();
    // First sub-pass: compute size of each op WITHOUT emitting bytes that
    // depend on label addresses (substitute zeros for now).
    let mut offsets: Vec<u64> = Vec::with_capacity(ops.len());
    let mut cursor: u64 = 0;
    for (_, op) in ops.iter() {
        offsets.push(cursor);
        cursor += op_len(op);
    }
    // Second sub-pass: resolve definitions to their final addresses.
    for ((_, op), off) in ops.iter().zip(offsets.iter()) {
        if let Op::LabelDef { name } = op {
            label_addrs.insert(name.clone(), org + *off);
        }
    }

    let mut instr_count: u32 = 0;
    for (line_no, op) in ops.iter() {
        match op {
            Op::MovRaxImm(imm) => {
                let v = resolve_mov_imm(imm, &label_addrs, *line_no)?;
                payload.extend_from_slice(&[0xB8]);
                payload.extend_from_slice(&v.to_le_bytes());
                instr_count += 1;
            }
            Op::MovRdiImm(imm) => {
                let v = resolve_mov_imm(imm, &label_addrs, *line_no)?;
                payload.extend_from_slice(&[0xBF]);
                payload.extend_from_slice(&v.to_le_bytes());
                instr_count += 1;
            }
            Op::MovRsiImm(imm) => {
                let v = resolve_mov_imm(imm, &label_addrs, *line_no)?;
                payload.extend_from_slice(&[0xBE]);
                payload.extend_from_slice(&v.to_le_bytes());
                instr_count += 1;
            }
            Op::IntImm(v) => {
                payload.extend_from_slice(&[0xCD, *v]);
                instr_count += 1;
            }
            Op::BitwiseRR { opcode, reg_dst, reg_src } => {
                let modrm = 0xC0 | (reg_src << 3) | reg_dst;
                payload.extend_from_slice(&[*opcode, modrm]);
                instr_count += 1;
            }
            Op::DbBytes(bytes) => {
                payload.extend_from_slice(bytes);
                instr_count += 1;
            }
            Op::LabelDef { .. } => {}
        }
    }

    let entry_addr = *label_addrs
        .get(&entry_label)
        .ok_or_else(|| anyhow!("[E_ASM_051_ENTRY_UNDEF] entry label `{}` undefined", entry_label))?;

    Ok(Module {
        org,
        entry_label,
        entry_addr,
        payload,
        instr_count,
    })
}

fn op_len(op: &Op) -> u64 {
    match op {
        Op::MovRaxImm(_) | Op::MovRdiImm(_) | Op::MovRsiImm(_) => 5,
        Op::IntImm(_) => 2,
        Op::BitwiseRR { .. } => 2,
        Op::DbBytes(b) => b.len() as u64,
        Op::LabelDef { .. } => 0,
    }
}

fn resolve_mov_imm(
    imm: &MovImm,
    label_addrs: &HashMap<String, u64>,
    line_no: usize,
) -> Result<u32> {
    Ok(match imm {
        MovImm::Literal(v) => *v as u32,
        MovImm::Label(name) => {
            let addr = label_addrs.get(name).ok_or_else(|| {
                anyhow!(
                    "[E_ASM_052_LABEL_RESOLVE] line {}: label `{}` not found",
                    line_no + 1,
                    name
                )
            })?;
            *addr as u32
        }
    })
}

fn parse_mov_imm(
    operand: &str,
    full_src: &str,
    labels: &HashMap<String, ()>,
) -> Result<MovImm> {
    if let Ok(v) = parse_int(operand) {
        return Ok(MovImm::Literal(v));
    }
    let trimmed = operand.trim();
    if labels.contains_key(trimmed) || label_will_be_defined(full_src, trimmed) {
        return Ok(MovImm::Label(trimmed.to_string()));
    }
    bail!("[E_ASM_018_OPERAND] unknown mov operand `{}`", operand)
}

fn label_will_be_defined(src: &str, name: &str) -> bool {
    src.lines()
        .any(|l| l.trim().strip_suffix(':').map(|s| s.trim() == name).unwrap_or(false))
}

fn split2(s: &str) -> Result<(&str, &str)> {
    let comma = s
        .find(',')
        .ok_or_else(|| anyhow!("[E_ASM_060_SPLIT2] expected `,` in operand list `{}`", s))?;
    let a = s[..comma].trim();
    let b = s[comma + 1..].trim();
    Ok((a, b))
}

fn strip_comment(line: &str) -> &str {
    if let Some(idx) = line.find(';') {
        &line[..idx]
    } else if let Some(idx) = line.find('#') {
        &line[..idx]
    } else {
        line
    }
}

fn parse_int(s: &str) -> Result<u64> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(rest, 16)
            .map_err(|_| anyhow!("[E_ASM_070_INT_HEX] invalid hex `{}`", s))
    } else {
        s.parse::<u64>()
            .map_err(|_| anyhow!("[E_ASM_071_INT_DEC] invalid integer `{}`", s))
    }
}

fn parse_db(rest: &str) -> Result<Vec<u8>> {
    let rest = rest.trim();
    if let Some(stripped) = rest
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
    {
        return Ok(stripped.bytes().collect());
    }
    let mut out = Vec::new();
    for tok in rest.split(',') {
        let v = parse_int(tok.trim())?;
        if v > 0xFF {
            bail!("[E_ASM_072_DB_BYTE] db byte too large `{}`", tok);
        }
        out.push(v as u8);
    }
    Ok(out)
}

fn parse_reg32(s: &str) -> Result<u8> {
    Ok(match s.trim().to_ascii_lowercase().as_str() {
        "eax" => 0,
        "ecx" => 1,
        "edx" => 2,
        "ebx" => 3,
        "esp" => 4,
        "ebp" => 5,
        "esi" => 6,
        "edi" => 7,
        other => bail!("[E_ASM_080_REG32] unknown 32-bit register `{}`", other),
    })
}

fn build_elf(module: &Module) -> Result<Vec<u8>> {
    let payload = &module.payload;
    let entry = module.entry_addr;
    let vaddr = module.org;
    let filesz = payload.len() as u64;
    let memsz = filesz;

    let mut out = Vec::new();
    // Elf64Ehdr
    out.extend_from_slice(&[
        0x7F, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ]);
    out.extend_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    out.extend_from_slice(&0x3Eu16.to_le_bytes()); // EM_X86_64
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&entry.to_le_bytes());
    out.extend_from_slice(&64u64.to_le_bytes()); // e_phoff
    out.extend_from_slice(&0u64.to_le_bytes()); // e_shoff
    out.extend_from_slice(&0u32.to_le_bytes()); // e_flags
    out.extend_from_slice(&64u16.to_le_bytes()); // e_ehsize
    out.extend_from_slice(&56u16.to_le_bytes()); // e_phentsize
    out.extend_from_slice(&1u16.to_le_bytes()); // e_phnum
    out.extend_from_slice(&0u16.to_le_bytes()); // e_shentsize
    out.extend_from_slice(&0u16.to_le_bytes()); // e_shnum
    out.extend_from_slice(&0u16.to_le_bytes()); // e_shstrndx
    debug_assert_eq!(out.len(), 64);

    // Elf64Phdr
    out.extend_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    out.extend_from_slice(&7u32.to_le_bytes()); // PF_R | PF_W | PF_X
    out.extend_from_slice(&120u64.to_le_bytes()); // p_offset
    out.extend_from_slice(&vaddr.to_le_bytes()); // p_vaddr
    out.extend_from_slice(&vaddr.to_le_bytes()); // p_paddr
    out.extend_from_slice(&filesz.to_le_bytes());
    out.extend_from_slice(&memsz.to_le_bytes());
    out.extend_from_slice(&1u64.to_le_bytes()); // p_align
    debug_assert_eq!(out.len(), 120);

    out.extend_from_slice(payload);
    let _ = &module.entry_label;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This assembler produces the files the kernel loads and runs in ring 3. It had no
    /// tests at all, which made it the least covered of the host crates
    /// with none: a parser that silently mis-assembles produces a program that fails
    /// inside a kernel, where there is nothing to report it.
    fn asm(src: &str) -> Module {
        parse(src).expect("source should assemble")
    }

    #[test]
    fn an_empty_source_is_refused_rather_than_assembled_to_nothing() {
        // A zero-byte program would be loaded, jumped into, and execute whatever
        // follows it in memory.
        assert!(parse("").is_err());
    }

    #[test]
    fn the_entry_directive_decides_where_execution_starts() {
        let m = asm("org 0x40010000\nentry start\nstart:\n  int 0x80\n");
        assert_eq!(m.org, 0x4001_0000);
        assert_eq!(m.entry_label, "start");
        assert_eq!(m.entry_addr, 0x4001_0000, "the first label sits at org");
    }

    #[test]
    fn a_label_after_two_bytes_is_two_bytes_past_org() {
        // `int 0x80` is two bytes: 0xCD 0x80. The second label must land after them.
        let m = asm("entry second\n  int 0x80\nsecond:\n  int 0x80\n");
        assert_eq!(m.entry_addr, DEFAULT_ORG + 2);
        assert_eq!(m.payload, vec![0xCD, 0x80, 0xCD, 0x80]);
    }

    #[test]
    fn an_entry_naming_a_label_that_does_not_exist_is_refused() {
        // Otherwise the ELF would carry an entry point pointing at nothing.
        assert!(parse("entry nowhere\nhere:\n  int 0x80\n").is_err());
    }

    #[test]
    fn db_puts_the_bytes_it_is_given_in_order() {
        let m = asm("entry s\ns:\n  db \"hi\"\n  db 0\n");
        assert_eq!(m.payload, vec![b'h', b'i', 0]);
    }

    #[test]
    fn a_comment_does_not_become_an_instruction() {
        let bare = asm("entry s\ns:\n  int 0x80\n");
        let commented = asm("; a note\nentry s\ns:\n  int 0x80  ; another\n");
        assert_eq!(bare.payload, commented.payload);
        assert_eq!(bare.instr_count, commented.instr_count);
    }

    #[test]
    fn a_word_that_is_not_an_instruction_is_refused_not_skipped() {
        // Skipping it would assemble a program missing a step, which is worse than
        // refusing to assemble at all.
        assert!(parse("entry s\ns:\n  frobnicate rax\n").is_err());
    }
}
