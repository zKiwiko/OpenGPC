#![allow(dead_code)]

use crate::codegen::bytecode::{Instruction, Opcode, Value};
use std::convert::TryFrom;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickInterval(Duration);

impl TickInterval {
    /// Convert a GPC `vm_tctrl` value (-9 ..= 40) to a [`TickInterval`].
    pub fn from_vm_tctrl(n: i8) -> Self {
        let ms = ((n as i16) + 10).clamp(1, 40) as u64;
        Self(Duration::from_millis(ms))
    }

    pub fn from_duration(d: Duration) -> Self {
        Self(d)
    }

    pub fn as_duration(self) -> Duration {
        self.0
    }
}

impl Default for TickInterval {
    /// 10 ms — the GPC default (`vm_tctrl(0)`).
    fn default() -> Self {
        Self::from_vm_tctrl(0)
    }
}

/// A compiled GPC script, ready to be loaded into the VM.
///
/// `init_fn` and `main_fn` are indices into `functions`. The VM prepends a
/// bootstrap preamble that calls `init` once then loops `main` indefinitely.
/// All code outside those two blocks (function definitions, globals) lives in
/// `code`; all function entry points in `functions` are relative to the start
/// of `code` and are automatically adjusted for the preamble by `VM::from_script`.
pub struct Script {
    /// Bytecode for all function bodies (function entry indices are into this vec).
    pub code: Vec<Instruction>,
    pub constants: Vec<Value>,
    pub strings: Vec<String>,
    pub functions: Vec<FuncProto>,
    /// Function table index of the `init {}` block — runs once at startup.
    pub init_fn: Option<u16>,
    /// Function table index of the `main {}` block — looped until `Halt`.
    pub main_fn: Option<u16>,
    /// How long to pause between `main` iterations.
    /// `None` means run as fast as possible (useful for tests and tooling).
    pub tick_interval: Option<TickInterval>,
}

/// A function prototype — a named entry point in the shared code stream.
#[derive(Debug, Clone)]
pub struct FuncProto {
    /// Optional name (string table index), used for debugging / disassembly.
    pub name: Option<u32>,
    /// Index of the first instruction for this function in `VM::code`.
    pub entry: usize,
    /// Number of parameters (arguments) the function accepts.
    pub arity: u8,
}

/// Saved caller state pushed onto the call stack when entering a function.
#[derive(Debug, Clone)]
struct CallFrame {
    /// IP to resume after `Return`.
    return_ip: usize,
    /// The caller's `window_base` — restored on return.
    caller_window: usize,
    /// Absolute flat-register index where return values should be written.
    /// Equal to `caller_window + A` of the `Call` instruction (the func slot).
    ret_base: usize,
    /// How many return values the caller expects (C operand of `Call`).
    retc: u8,
}

pub struct VM {
    /// Flat 256-register file, addressed as `registers[window_base + local_index]`.
    pub registers: [Value; 256],
    /// Constant pool — loaded at startup, never mutated by running code.
    pub constants: Vec<Value>,
    /// String table; `Value::Str(i)` resolves to `strings[i as usize]`.
    pub strings: Vec<String>,
    /// Global variable table, indexed by a u16 key.
    pub globals: Vec<Value>,
    /// Array heap; `Value::Array(i)` resolves to `arrays[i as usize]`.
    pub arrays: Vec<Vec<Value>>,
    /// Function table; `Value::Func(i)` resolves to `functions[i as usize]`.
    pub functions: Vec<FuncProto>,
    /// The instruction stream (function bodies and main code share one flat vec).
    pub code: Vec<Instruction>,
    /// Instruction pointer.
    pub ip: usize,
    /// Base offset into `registers` for the current call frame's locals.
    ///
    /// Local register `Rn` always means `self.registers[self.window_base + n]`.
    /// The top-level frame has `window_base = 0`.
    window_base: usize,
    /// Call stack — grows on `Call`, shrinks on `Return`.
    call_stack: Vec<CallFrame>,
}

impl VM {
    pub fn new(
        code: Vec<Instruction>,
        constants: Vec<Value>,
        strings: Vec<String>,
        functions: Vec<FuncProto>,
    ) -> Self {
        Self {
            registers: [Value::Nil; 256],
            constants,
            strings,
            globals: Vec::new(),
            arrays: Vec::new(),
            functions,
            code,
            ip: 0,
            window_base: 0,
            call_stack: Vec::new(),
        }
    }

    /// Construct a VM from a compiled [`Script`].
    ///
    /// A bootstrap preamble is prepended to the code stream:
    ///
    /// ```text
    /// (both init and main)        (main only)           (init only)
    /// 0: LOADFUNC R0 F_init       0: LOADFUNC R0 F_main  0: LOADFUNC R0 F_init
    /// 1: CALL     R0 0 0          1: CALL     R0 0 0     1: CALL     R0 0 0
    /// 2: LOADFUNC R0 F_main  ←┐  2: JUMP     0      ←┘  2: HALT
    /// 3: CALL     R0 0 0      |
    /// 4: JUMP     2        ───┘
    /// ```
    ///
    /// R0 is used as a scratch register in the bootstrap frame (window_base 0).
    /// All `FuncProto::entry` values are shifted past the preamble automatically.
    ///
    /// Returns `(vm, main_loop_ip, tick_interval)`:
    /// - `main_loop_ip` — the preamble IP that starts each `main` iteration;
    ///   pass this to [`VM::run_script`] so it knows when to sleep.
    ///   `None` when there is no `main` block.
    /// - `tick_interval` — taken from [`Script::tick_interval`].
    pub fn from_script(mut script: Script) -> (Self, Option<usize>, Option<TickInterval>) {
        // Scratch register used exclusively during the bootstrap frame.
        const BR: u8 = 0;

        let mut main_loop_ip: Option<usize> = None;

        let preamble: Vec<Instruction> = match (script.init_fn, script.main_fn) {
            (Some(init_fi), Some(main_fi)) => {
                main_loop_ip = Some(2); // LOADFUNC for main is at preamble index 2
                vec![
                    Instruction::encode_ad(Opcode::LoadFunc, BR, init_fi),
                    Instruction::encode_abc(Opcode::Call, BR, 0, 0),
                    // 2: main_loop ← JUMP target
                    Instruction::encode_ad(Opcode::LoadFunc, BR, main_fi),
                    Instruction::encode_abc(Opcode::Call, BR, 0, 0),
                    Instruction::encode_ad(Opcode::Jump, 0, 2),
                ]
            }
            (None, Some(main_fi)) => {
                main_loop_ip = Some(0); // LOADFUNC for main is at preamble index 0
                vec![
                    Instruction::encode_ad(Opcode::LoadFunc, BR, main_fi),
                    Instruction::encode_abc(Opcode::Call, BR, 0, 0),
                    Instruction::encode_ad(Opcode::Jump, 0, 0),
                ]
            }
            (Some(init_fi), None) => vec![
                Instruction::encode_ad(Opcode::LoadFunc, BR, init_fi),
                Instruction::encode_abc(Opcode::Call, BR, 0, 0),
                Instruction::encode_abc(Opcode::Halt, 0, 0, 0),
            ],
            (None, None) => vec![],
        };

        // Shift every function entry point past the preamble.
        let offset = preamble.len();
        for f in &mut script.functions {
            f.entry += offset;
        }

        let tick_interval = script.tick_interval;

        let mut code = preamble;
        code.extend(script.code);

        (
            VM::new(code, script.constants, script.strings, script.functions),
            main_loop_ip,
            tick_interval,
        )
    }

    /// Run the VM until `ip` falls off the end of the code or a `Halt` is executed.
    ///
    /// This is the raw execution loop — it never sleeps and has no concept of
    /// `init`/`main` timing. Use [`VM::run_script`] for full script execution
    /// with proper tick-rate behaviour.
    pub fn run(&mut self) {
        loop {
            if self.ip >= self.code.len() {
                break;
            }
            let instr = self.code[self.ip];
            match self.execute(instr) {
                Some(new_ip) => self.ip = new_ip,
                None => self.ip += 1,
            }
        }
    }

    /// Run a script with Cronus/Titan-style tick timing.
    ///
    /// Executes the bootstrap preamble built by [`VM::from_script`]. When
    /// `main` returns to the preamble `JUMP`, execution pauses for
    /// [`Script::tick_interval`] before the next iteration. If `tick_interval`
    /// is `None` the loop runs without any delay.
    ///
    /// The `main_loop_ip` is the address of the `LOADFUNC` instruction in the
    /// preamble that starts each `main` iteration. The VM detects that the IP
    /// has reached this address *after* `main` returns and sleeps there.
    ///
    /// `stop` is called once per tick; return `true` to halt the loop (useful
    /// for signal handling or frame-budget enforcement in the host driver).
    pub fn run_script(
        &mut self,
        main_loop_ip: usize,
        tick_interval: Option<TickInterval>,
        mut stop: impl FnMut() -> bool,
    ) {
        loop {
            if self.ip >= self.code.len() {
                break;
            }

            // We've lapped back to the top of the main loop — apply the tick delay.
            if self.ip == main_loop_ip {
                if stop() {
                    break;
                }
                if let Some(interval) = tick_interval {
                    std::thread::sleep(interval.as_duration());
                }
            }

            let instr = self.code[self.ip];
            match self.execute(instr) {
                Some(new_ip) => self.ip = new_ip,
                None => self.ip += 1,
            }
        }
    }

    /// Translates a local register index to an absolute index in the flat register file.
    #[inline]
    fn reg(&self, idx: u8) -> usize {
        self.window_base + idx as usize
    }

    /// Execute one instruction.
    ///
    /// Returns `Some(new_ip)` when the instruction explicitly changes the IP
    /// (jumps, calls, returns, halt), or `None` to advance sequentially.
    fn execute(&mut self, instr: Instruction) -> Option<usize> {
        let op =
            Opcode::try_from(instr.opcode()).unwrap_or_else(|b| panic!("Unknown opcode: {b:#04x}"));

        match op {
            // ── Control flow ──────────────────────────────────────────────
            Opcode::Halt => return Some(self.code.len()),

            Opcode::Jump => return Some(instr.d() as usize),

            Opcode::JumpIf => {
                if self.is_truthy(self.registers[self.reg(instr.a())]) {
                    return Some(instr.d() as usize);
                }
            }

            Opcode::JumpIfNot => {
                if !self.is_truthy(self.registers[self.reg(instr.a())]) {
                    return Some(instr.d() as usize);
                }
            }

            // FORPREP A D
            //   Expects: R[A]=init, R[A+1]=limit, R[A+2]=step (all pre-loaded)
            //   R[A] -= R[A+2]  (pre-subtract so ForLoop adds it back before first use)
            //   ip = D          (D = absolute address of the FORLOOP instruction)
            Opcode::ForPrep => {
                let a = self.reg(instr.a());
                let step = self.as_int(self.registers[a + 2]);
                let counter = self.as_int(self.registers[a]);
                self.registers[a] = Value::Int(counter - step);
                return Some(instr.d() as usize);
            }

            // FORLOOP A D
            //   R[A] += R[A+2]
            //   if step >= 0: jump to D if R[A] <= R[A+1]
            //   if step <  0: jump to D if R[A] >= R[A+1]
            //   D = absolute address of the loop body start
            Opcode::ForLoop => {
                let a = self.reg(instr.a());
                let step = self.as_int(self.registers[a + 2]);
                let limit = self.as_int(self.registers[a + 1]);
                let new_counter = self.as_int(self.registers[a]) + step;
                self.registers[a] = Value::Int(new_counter);
                let keep_looping = if step >= 0 {
                    new_counter <= limit
                } else {
                    new_counter >= limit
                };
                if keep_looping {
                    return Some(instr.d() as usize);
                }
            }

            // TAILCALL Ra Argc Retc  (ABC)
            //
            //   Reuses the current call frame instead of pushing a new one, so the
            //   callee's Return goes directly back to our caller.
            //
            //   Args sitting at Ra+1..Ra+Argc are shifted down to window_base+0..
            //   so the callee sees them as its own R0, R1, ... with window_base unchanged.
            Opcode::TailCall => {
                let a = instr.a();
                let argc = instr.b() as usize;

                let func_abs = self.window_base + a as usize;
                let fi = match self.registers[func_abs] {
                    Value::Func(fi) => fi as usize,
                    other => panic!("TAILCALL: expected Func, got {other:?}"),
                };
                let entry = self.functions[fi].entry;

                // Shift args down into the start of the current window so the
                // callee sees them as R0..R(argc-1). The existing CallFrame on the
                // stack already carries the correct return address and caller window.
                for i in 0..argc {
                    self.registers[self.window_base + i] = self.registers[func_abs + 1 + i];
                }

                return Some(entry);
            }

            // ── Data movement ─────────────────────────────────────────────
            Opcode::LoadK => {
                self.registers[self.reg(instr.a())] = self.constants[instr.d() as usize];
            }

            Opcode::Move => {
                self.registers[self.reg(instr.a())] = self.registers[self.reg(instr.b())];
            }

            Opcode::LoadInt => {
                self.registers[self.reg(instr.a())] = Value::Int(instr.d() as i16 as i32);
            }

            Opcode::LoadBool => {
                self.registers[self.reg(instr.a())] = Value::Bool(instr.d() != 0);
            }

            Opcode::LoadNil => {
                self.registers[self.reg(instr.a())] = Value::Nil;
            }

            Opcode::LoadGlobal => {
                let val = self
                    .globals
                    .get(instr.d() as usize)
                    .copied()
                    .unwrap_or(Value::Nil);
                self.registers[self.reg(instr.a())] = val;
            }

            Opcode::StoreGlobal => {
                let idx = instr.d() as usize;
                let val = self.registers[self.reg(instr.a())];
                if idx >= self.globals.len() {
                    self.globals.resize(idx + 1, Value::Nil);
                }
                self.globals[idx] = val;
            }

            Opcode::LoadFunc => {
                self.registers[self.reg(instr.a())] = Value::Func(instr.d());
            }

            // ── Arithmetic ────────────────────────────────────────────────
            Opcode::AddVV => self.binop_vv(instr, |a, b| a + b),
            Opcode::AddVK => self.binop_vk(instr, |a, b| a + b),
            Opcode::SubVV => self.binop_vv(instr, |a, b| a - b),
            Opcode::SubVK => self.binop_vk(instr, |a, b| a - b),
            Opcode::MulVV => self.binop_vv(instr, |a, b| a * b),
            Opcode::MulVK => self.binop_vk(instr, |a, b| a * b),
            Opcode::DivVV => self.binop_vv(instr, |a, b| a / b),
            Opcode::DivVK => self.binop_vk(instr, |a, b| a / b),
            Opcode::ModVV => self.binop_vv(instr, |a, b| a % b),
            Opcode::ModVK => self.binop_vk(instr, |a, b| a % b),

            // ── Comparison ────────────────────────────────────────────────
            Opcode::EqVV => self.cmpop_vv(instr, |a, b| a == b),
            Opcode::EqVK => self.cmpop_vk(instr, |a, b| a == b),
            Opcode::LtVV => self.cmpop_vv(instr, |a, b| a < b),
            Opcode::LtVK => self.cmpop_vk(instr, |a, b| a < b),
            Opcode::LteVV => self.cmpop_vv(instr, |a, b| a <= b),
            Opcode::LteVK => self.cmpop_vk(instr, |a, b| a <= b),
            Opcode::GtVV => self.cmpop_vv(instr, |a, b| a > b),
            Opcode::GtVK => self.cmpop_vk(instr, |a, b| a > b),
            Opcode::GteVV => self.cmpop_vv(instr, |a, b| a >= b),
            Opcode::GteVK => self.cmpop_vk(instr, |a, b| a >= b),
            Opcode::NotV => {
                let b = self.is_truthy(self.registers[self.reg(instr.b())]);
                self.registers[self.reg(instr.a())] = Value::Bool(!b);
            }
            Opcode::NegV => {
                let b = self.as_int(self.registers[self.reg(instr.b())]);
                self.registers[self.reg(instr.a())] = Value::Int(-b);
            }
            Opcode::BitNotV => {
                let b = self.as_int(self.registers[self.reg(instr.b())]);
                self.registers[self.reg(instr.a())] = Value::Int(!b);
            }
            Opcode::AndVV => self.binop_vv(instr, |a, b| a & b),
            Opcode::AndVK => self.binop_vk(instr, |a, b| a & b),
            Opcode::OrVV => self.binop_vv(instr, |a, b| a | b),
            Opcode::OrVK => self.binop_vk(instr, |a, b| a | b),
            Opcode::XorVV => self.binop_vv(instr, |a, b| a ^ b),
            Opcode::XorVK => self.binop_vk(instr, |a, b| a ^ b),
            Opcode::ShlVV => self.binop_vv(instr, |a, b| a << b),
            Opcode::ShlVK => self.binop_vk(instr, |a, b| a << b),
            Opcode::ShrVV => self.binop_vv(instr, |a, b| a >> b),
            Opcode::ShrVK => self.binop_vk(instr, |a, b| a >> b),
            Opcode::SarVV => self.binop_vv(instr, |a, b| a >> b), // i32 >> i32 is arithmetic in Rust
            Opcode::SarVK => self.binop_vk(instr, |a, b| a >> b),

            Opcode::GetIndex => {
                let a = self.reg(instr.a());
                let b = self.reg(instr.b());
                let c = self.reg(instr.c());
                match self.registers[b] {
                    Value::Str(si) => {
                        let s = self
                            .strings
                            .get(si as usize)
                            .map(String::as_str)
                            .unwrap_or("<invalid string index>");
                        let idx = match self.registers[c] {
                            Value::Int(n) => n as usize,
                            other => panic!("GETINDEX: expected Int index for Str, got {other:?}"),
                        };
                        let ch = s.chars().nth(idx).unwrap_or('\0');
                        self.registers[a] = Value::Int(ch as u32 as i32);
                    }
                    Value::Array(ai) => {
                        let idx = match self.registers[c] {
                            Value::Int(n) => n as usize,
                            other => {
                                panic!("GETINDEX: expected Int index for Array, got {other:?}")
                            }
                        };
                        let result = self
                            .arrays
                            .get(ai as usize)
                            .and_then(|arr| arr.get(idx).copied())
                            .unwrap_or(Value::Nil);
                        self.registers[a] = result;
                    }
                    other => panic!("GETINDEX: expected Str or Array, got {other:?}"),
                }
            }
            Opcode::SetIndex => {
                let a = self.reg(instr.a());
                let b = self.reg(instr.b());
                let c = self.reg(instr.c());
                match self.registers[a] {
                    Value::Str(si) => {
                        let s = self
                            .strings
                            .get_mut(si as usize)
                            .unwrap_or_else(|| panic!("SETINDEX: invalid string index {si}"));
                        let idx = match self.registers[b] {
                            Value::Int(n) => n as usize,
                            other => panic!("SETINDEX: expected Int index for Str, got {other:?}"),
                        };
                        let ch = match self.registers[c] {
                            Value::Int(n) => std::char::from_u32(n as u32).unwrap_or('\0'),
                            other => {
                                panic!("SETINDEX: expected Int char code for Str, got {other:?}")
                            }
                        };
                        if idx < s.len() {
                            let mut chars: Vec<char> = s.chars().collect();
                            chars[idx] = ch;
                            *s = chars.into_iter().collect();
                        }
                    }
                    Value::Array(ai) => {
                        let idx = match self.registers[b] {
                            Value::Int(n) => n as usize,
                            other => {
                                panic!("SETINDEX: expected Int index for Array, got {other:?}")
                            }
                        };
                        let val = self.registers[c];
                        let arr = self
                            .arrays
                            .get_mut(ai as usize)
                            .unwrap_or_else(|| panic!("SETINDEX: invalid array index {ai}"));
                        if idx < arr.len() {
                            arr[idx] = val;
                        }
                    }
                    other => panic!("SETINDEX: expected Str or Array, got {other:?}"),
                }
            }

            // NEWARRAY A D
            //   Allocates a new array of D Nil slots on the array heap.
            //   registers[A] = Value::Array(heap_index)
            Opcode::NewArray => {
                let size = instr.d() as usize;
                let idx = self.arrays.len() as u32;
                self.arrays.push(vec![Value::Nil; size]);
                self.registers[self.reg(instr.a())] = Value::Array(idx);
            }

            // COPYRANGE A B C  (ABC)
            //   Copies C registers starting at R[B] into R[A]..R[A+C-1].
            //   C is a raw count, not a register index.
            Opcode::CopyRange => {
                let a = self.reg(instr.a());
                let b = self.reg(instr.b());
                let count = instr.c() as usize;
                for i in 0..count {
                    self.registers[a + i] = self.registers[b + i];
                }
            }

            Opcode::LenV => {
                let b = self.reg(instr.b());
                match self.registers[b] {
                    Value::Str(si) => {
                        let len = self
                            .strings
                            .get(si as usize)
                            .map(|s| s.chars().count())
                            .unwrap_or(0);
                        self.registers[self.reg(instr.a())] = Value::Int(len as i32);
                    }
                    Value::Array(ai) => {
                        let len = self
                            .arrays
                            .get(ai as usize)
                            .map(|arr| arr.len())
                            .unwrap_or(0);
                        self.registers[self.reg(instr.a())] = Value::Int(len as i32);
                    }
                    other => panic!("LENV: expected Str or Array, got {other:?}"),
                }
            }

            Opcode::LenK => {
                let d = instr.d() as usize;
                match self.constants.get(d) {
                    Some(Value::Str(si)) => {
                        let s = self
                            .strings
                            .get(*si as usize)
                            .map(String::as_str)
                            .unwrap_or("<invalid string index>");
                        self.registers[self.reg(instr.a())] = Value::Int(s.chars().count() as i32);
                    }
                    Some(other) => panic!("LENK: expected Str constant, got {other:?}"),
                    None => panic!("LENK: invalid constant index {d}"),
                }
            }

            Opcode::ConcatVV => {
                let b = self.reg(instr.b());
                let c = self.reg(instr.c());
                let s1 = match self.registers[b] {
                    Value::Str(si) => self
                        .strings
                        .get(si as usize)
                        .map(String::as_str)
                        .unwrap_or("<invalid string index>"),
                    other => panic!("CONCATVV: expected Str, got {other:?}"),
                };
                let s2 = match self.registers[c] {
                    Value::Str(si) => self
                        .strings
                        .get(si as usize)
                        .map(String::as_str)
                        .unwrap_or("<invalid string index>"),
                    other => panic!("CONCATVV: expected Str, got {other:?}"),
                };
                let result = format!("{}{}", s1, s2);
                let idx = self.strings.len() as u32;
                self.strings.push(result);
                self.registers[self.reg(instr.a())] = Value::Str(idx);
            }

            Opcode::ConcatVK => {
                let b = self.reg(instr.b());
                let c = instr.c() as usize;
                let s1 = match self.registers[b] {
                    Value::Str(si) => self
                        .strings
                        .get(si as usize)
                        .map(String::as_str)
                        .unwrap_or("<invalid string index>"),
                    other => panic!("CONCATVK: expected Str, got {other:?}"),
                };
                let s2 = match self.constants.get(c) {
                    Some(Value::Str(si)) => self
                        .strings
                        .get(*si as usize)
                        .map(String::as_str)
                        .unwrap_or("<invalid string index>"),
                    Some(other) => panic!("CONCATVK: expected Str constant, got {other:?}"),
                    None => panic!("CONCATVK: invalid constant index {c}"),
                };
                let result = format!("{}{}", s1, s2);
                let idx = self.strings.len() as u32;
                self.strings.push(result);
                self.registers[self.reg(instr.a())] = Value::Str(idx);
            }

            // ── Functions ─────────────────────────────────────────────────
            //
            // CALL Ra Argc Retc  (ABC)
            //
            //   Ra        = register holding a Value::Func
            //   Ra+1..    = argument registers; the callee sees these as R0, R1, ...
            //   Retc      = number of return values the caller wants back in Ra..
            //
            // On entry the register window is shifted so `window_base = func_abs + 1`,
            // meaning callee R0 == caller Ra+1 in the flat register file.
            Opcode::Call => {
                let a = instr.a();
                let _argc = instr.b(); // reserved for future arity validation
                let retc = instr.c();

                let func_abs = self.window_base + a as usize;
                let fi = match self.registers[func_abs] {
                    Value::Func(fi) => fi as usize,
                    other => panic!("CALL: expected Func, got {other:?}"),
                };
                let entry = self.functions[fi].entry;

                self.call_stack.push(CallFrame {
                    return_ip: self.ip + 1,
                    caller_window: self.window_base,
                    ret_base: func_abs, // return values overwrite the func slot
                    retc,
                });

                // Callee's R0 is the first argument, sitting right above the func slot.
                self.window_base = func_abs + 1;
                return Some(entry);
            }

            // RETURN Ra Retc  (AB)
            //
            //   Copies min(caller_retc, Retc) values starting at callee's Ra
            //   back into the caller's function-slot (ret_base), then restores
            //   the caller's window and resumes at return_ip.
            Opcode::Return => {
                let a = instr.a();
                let retc = instr.b();

                match self.call_stack.pop() {
                    None => return Some(self.code.len()), // top-level return — halt
                    Some(frame) => {
                        let actual_retc = frame.retc.min(retc) as usize;
                        for i in 0..actual_retc {
                            let src = self.window_base + a as usize + i;
                            let dst = frame.ret_base + i;
                            self.registers[dst] = self.registers[src];
                        }
                        self.window_base = frame.caller_window;
                        return Some(frame.return_ip);
                    }
                }
            }

            Opcode::Printf => match self.registers[self.reg(instr.a())] {
                Value::Str(i) => {
                    let s = self
                        .strings
                        .get(i as usize)
                        .map(String::as_str)
                        .unwrap_or("<invalid string index>");
                    println!("{s}");
                }
                Value::Int(n) => println!("{n}"),
                Value::Bool(b) => println!("{b}"),
                other => panic!("PRINTF: expected Str, Int, Bool, got {other:?}"),
            },
            Opcode::Nop => {
                // Do nothing :p
            }
        }

        None
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    #[inline]
    fn is_truthy(&self, v: Value) -> bool {
        match v {
            Value::Nil => false,
            Value::Bool(b) => b,
            Value::Int(n) => n != 0,
            Value::Func(_) => true,
            Value::Array(_) => true,
            Value::Str(i) => !self
                .strings
                .get(i as usize)
                .map(|s| s.is_empty())
                .unwrap_or(true),
        }
    }

    #[inline]
    fn as_int(&self, v: Value) -> i32 {
        match v {
            Value::Int(n) => n,
            other => panic!("(i) AS_INT: Expected Int, got {other:?}"),
        }
    }

    #[inline]
    fn binop_vv(&mut self, instr: Instruction, op: fn(i32, i32) -> i32) {
        let b = self.as_int(self.registers[self.reg(instr.b())]);
        let c = self.as_int(self.registers[self.reg(instr.c())]);
        self.registers[self.reg(instr.a())] = Value::Int(op(b, c));
    }

    #[inline]
    fn binop_vk(&mut self, instr: Instruction, op: fn(i32, i32) -> i32) {
        let b = self.as_int(self.registers[self.reg(instr.b())]);
        let c = self.as_int(self.constants[instr.c() as usize]);
        self.registers[self.reg(instr.a())] = Value::Int(op(b, c));
    }

    #[inline]
    fn cmpop_vv(&mut self, instr: Instruction, op: fn(i32, i32) -> bool) {
        let b = self.as_int(self.registers[self.reg(instr.b())]);
        let c = self.as_int(self.registers[self.reg(instr.c())]);
        self.registers[self.reg(instr.a())] = Value::Bool(op(b, c));
    }

    #[inline]
    fn cmpop_vk(&mut self, instr: Instruction, op: fn(i32, i32) -> bool) {
        let b = self.as_int(self.registers[self.reg(instr.b())]);
        let c = self.as_int(self.constants[instr.c() as usize]);
        self.registers[self.reg(instr.a())] = Value::Bool(op(b, c));
    }
}
