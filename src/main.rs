mod codegen;
mod vm;

use vm::{Script, TickInterval, VM};

fn main() {
    // Placeholder: the compiler will eventually produce a Script from source.
    // The host driver pattern for a real script looks like this:
    //
    //   let script = compiler::compile(source);
    //   let (mut vm, main_loop_ip, tick_interval) = VM::from_script(script);
    //   vm.run_script(main_loop_ip.unwrap_or(0), tick_interval, || false);
    //
    // For now, just confirm the default tick interval is 10 ms (vm_tctrl(0)).
    let default_tick = TickInterval::default();
    assert_eq!(
        default_tick.as_duration(),
        std::time::Duration::from_millis(10)
    );
    println!(
        "OpenGPC VM ready. Default tick interval: {:?}",
        default_tick.as_duration()
    );
}
