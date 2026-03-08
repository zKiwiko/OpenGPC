#![allow(dead_code)]

use std::convert::TryFrom;

/// A 32-bit encoded instruction.
///
/// Two encoding formats are supported:
///
/// ```text
/// ABC  — bits: [31..24 = C] [23..16 = B] [15..8 = A] [7..0 = opcode]
/// AD   — bits: [31..16 = D] [15..8 = A]  [7..0 = opcode]
/// ```
///
/// `D` is a 16-bit wide operand that spans the `B` and `C` fields and is
/// used for constant indices, jump targets, and other wide immediates.
#[derive(Clone, Copy, Debug)]
pub struct Instruction(u32);

impl Instruction {
    /// Construct from a raw `u32` (e.g. when reading from a binary).
    #[inline]
    pub fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// Encode an **ABC** format instruction.
    #[inline]
    pub fn encode_abc(op: Opcode, a: u8, b: u8, c: u8) -> Self {
        Self((op as u32) | ((a as u32) << 8) | ((b as u32) << 16) | ((c as u32) << 24))
    }

    /// Encode an **AD** format instruction.
    /// `d` occupies the upper 16 bits (spanning the B and C fields).
    #[inline]
    pub fn encode_ad(op: Opcode, a: u8, d: u16) -> Self {
        Self((op as u32) | ((a as u32) << 8) | ((d as u32) << 16))
    }

    #[inline]
    pub fn opcode(self) -> u8 {
        (self.0 & 0xFF) as u8
    }

    #[inline]
    pub fn a(self) -> u8 {
        ((self.0 >> 8) & 0xFF) as u8
    }

    #[inline]
    pub fn b(self) -> u8 {
        ((self.0 >> 16) & 0xFF) as u8
    }

    #[inline]
    pub fn c(self) -> u8 {
        ((self.0 >> 24) & 0xFF) as u8
    }

    /// The 16-bit wide `D` operand (spans the `B` and `C` fields).
    #[inline]
    pub fn d(self) -> u16 {
        ((self.0 >> 16) & 0xFFFF) as u16
    }
}

/// Opcodes for the OpenGPC virtual machine.
///
/// Operand conventions:
///   `V` — register (variable) index  
///   `K` — constant-pool index  
///   `D` — 16-bit wide immediate / address
///
/// Instruction formats:
///   `ABC` — `opcode | A<<8 | B<<16 | C<<24`  
///   `AD`  — `opcode | A<<8 | D<<16`
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opcode {
    // ── Control flow ─────────────────────────────
    Halt,      // stop execution
    Jump,      // AD: ip = D
    JumpIf,    // AD: if registers[A] is truthy  → ip = D
    JumpIfNot, // AD: if registers[A] is falsy   → ip = D
    ForPrep,   // AD: registers[A] -= D; ip += Absolute(D)
    ForLoop,   // AD: registers[A] += D; if registers[A] < registers[A+1] then ip += Absolute(D)
    TailCall,  // ABC: A = func reg, B = argc, C = retc; like CALL but does not return to caller

    // ── Data movement ────────────────────────────
    LoadK,       // AD:  registers[A] = constants[D]
    Move,        // AB:  registers[A] = registers[B]
    LoadGlobal,  // AD:  registers[A] = globals[D]
    StoreGlobal, // AD:  globals[D]   = registers[A]
    LoadFunc,    // AD:  registers[A] = Value::Func(D)  (D = function table index)
    LoadInt,     // AD:  registers[A] = Value::Int(D)   (D = immediate integer value)
    LoadBool,    // AD:  registers[A] = Value::Bool(D != 0) (D = 0 or 1)
    LoadNil,     // A:   registers[A] = Value::Nil
    GetIndex,    // ABC: registers[A] = registers[B][registers[C]]
    SetIndex,    // ABC: registers[A][registers[B]] = registers[C]
    NewArray,    // AD:  registers[A] = new array of size D
    CopyRange,   // AD:  copy registers[A..A+D-1] to registers[A+1..A+D]
    LenV,        // AB:  registers[A] = len(registers[B])
    LenK,        // AD:  registers[A] = len(constants[D])
    ConcatVV,    // ABC: registers[A] = registers[B] .. registers[C]
    ConcatVK,    // ABC: registers[A] = registers[B] .. constants[C]
    // ── Arithmetic (ABC) ─────────────────────────
    AddVV, // registers[A] = registers[B] + registers[C]
    AddVK, // registers[A] = registers[B] + constants[C]
    SubVV,
    SubVK,
    MulVV,
    MulVK,
    DivVV,
    DivVK,
    ModVV,
    ModVK,

    // ── Comparison (ABC, result in A) ─────────────
    EqVV,
    EqVK,
    LtVV,
    LtVK,
    LteVV,
    LteVK,
    GtVV,
    GtVK,
    GteVV,
    GteVK,

    // -- Unary operations (AB) ─────────────────────────
    NotV,    // registers[A] = !registers[B]
    NegV,    // registers[A] = -registers[B]
    BitNotV, // registers[A] = ~registers[B]

    // -- Bitwise operations (ABC) ─────────────────────────
    AndVV, // registers[A] = registers[B] & registers[C]
    AndVK, // registers[A] = registers[B] & constants[C]
    OrVV,  // registers[A] = registers[B] | registers[C]
    OrVK,  // registers[A] = registers[B] | constants[C]
    XorVV, // registers[A] = registers[B] ^ registers[C]
    XorVK, // registers[A] = registers[B] ^ constants[C]
    ShlVV, // registers[A] = registers[B] << registers[C]
    ShlVK, // registers[A] = registers[B] << constants[C]
    ShrVV, // registers[A] = registers[B] >> registers[C]
    ShrVK, // registers[A] = registers[B] >> constants[C]
    SarVV, // registers[A] = registers[B] >> registers[C] (arithmetic shift)
    SarVK, // registers[A] = registers[B] >> constants[C] (arithmetic

    // ── Functions ────────────────────────────────────────
    //
    // Calling convention (register windowing):
    //   CALL Ra Argc Retc  (ABC)
    //     Ra              = register holding a Value::Func
    //     Ra+1 .. Ra+Argc = arguments; callee sees them as R0 .. R(Argc-1)
    //     On return, Retc values are written back starting at Ra.
    //
    //   RETURN Ra Retc  (AB)
    //     Copies Retc values starting at callee's Ra back to the caller's
    //     function slot (which becomes the first return-value slot).
    Call,   // ABC: A = func reg, B = argc, C = retc
    Return, // AB:  A = first result reg, B = retc

    // OpenGPC Specific Instructions
    Printf,
    // Compatibility Specific Instructions
    // These are based on speculation of what properietary GPC
    //  compilers might produce, and may be removed or changed in the future.
    Nop, // do nothing
}

impl TryFrom<u8> for Opcode {
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if value <= Opcode::Nop as u8 {
            // SAFETY: All variants are contiguous starting from 0 (`repr(u8)`),
            // and `value` has been checked to be within the valid range.
            Ok(unsafe { std::mem::transmute(value) })
        } else {
            Err(value)
        }
    }
}

/// A value that can live in a register or the constant pool.
#[derive(Clone, Copy, Debug)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i32),
    Str(u32),   // index into the VM's string table
    Func(u16),  // index into the VM's function table
    Array(u32), // index into the VM's array heap
}
