## Disclaimer

This project is an independent, clean-room implementation inspired by the GPC scripting language. It is not affiliated with, endorsed by, or associated with Titan, Cronus, Collective Minds, ConsoleTuner or its developers. The compiler, bytecode format, and virtual machine in this repository were written entirely from scratch and do not contain any reverse-engineered code, proprietary assets, or confidential documentation from Cronus software or devices. No components of this project interact with Cronus hardware, firmware, or official tooling. The implementation is based solely on publicly available information and independent design decisions.
Cronus, ConsoleTuner, Cronus Zen are trademarks of their respective owners. This project is not affiliated with or endorsed by them.

# About OpenGPC

OpenGPC is an independent, community-driven project that provides a development toolkit, compiler, and
virtual machine/simulator for GPC Scripting Language. OpenGPC focuses on providing tools for experimenting
with, analyzing and debugging GPC Scripts without needing external hardware. OpenGPC allows developers
to compile and run GPC Scripts inside a sandboxed runtime.

OpenGPC primarily targets Collective Mind's GPC syntax as its the most widly used variant but eventually
will support other variants like ConsoleTuner's implementation.

OpenGPC is designed as a standalone implementation and does not interact with proprietary hardware, reverse-engineered components, or external device firmware. OpenGPC binaries are intended for use with OpenGPC itself and projects that adopt its bytecode format and design specifications.

OpenGPC is written fully in Rust.

## Compiler

OpenGPC compiles GPC code into its own bytecode format - loosely inspired by LuaJIT and CPython - using
specialized Opcodes for comparisons, arthmetic, reading, and loading from registers or constants specifically.

OpenGPC's compiler allows you to output your compiled code in an assembly-like language in order to
help developers understand their code. Alternatively, you can also input a binary and get that same output.

## Virtual Machine

OpenGPC uses a register based virtual machine to interpret its binaries.
Its specialized opcodes for different types of operations help it be as fast as it possibly
can - the less instructions the better.

Since OpenGPC isnt designed to run on embedded hardware like other implementations of the
language, OpenGPC is able to get array with more intensive operations and tricks in
general compared to others. A couple examples would be mutable arrays, floating point numbers,
and locally-scoped variables.

## Simulation

OpenGPC's simulator has some important limitations to consider. Since OpenGPC runs on general-purpose hardware rather than embedded systems, its timing reports may differ from other implementations. However, the simulator is designed to be as accurate as possible within these constraints.

Instead of interacting with real hardware, games, or your computer - scripts are executed in a sandboxed
Virtual Machine, where their behavior can be inspected and analyzed. This allows developers to Trace
script execution, inspect register values and variables, view combo timing and control flow, debug
logical errors.
