// We want to be able to run
//
//     $ cargo rtic-trace --bin blinky --serial /dev/ttyUSB3
//

use std::env;
use structopt::StructOpt;

mod building;

#[derive(Debug, StructOpt)]
#[structopt(name = "cargo-rtic-trace", about = "TODO")]
struct Opt {
    /// Binary to flash and trace.
    #[structopt(long = "bin")]
    bin: String,

    // TODO handle example (or forward unknown arguments to rustc)

    /// Serial device over which trace stream is expected.
    #[structopt(long = "serial")]
    serial: String,
}

fn main() {
    let opt = Opt::from_iter(env::args().skip(1));

    // build wanted binary
    let artifact = building::cargo_build(&opt.bin);
    println!("{:?}", artifact);

    // TODO ensure --serial is properly configured

    // TODO resolve the data we need from RTIC app decl.

    // TODO flash binary with probe.rs

    // TODO properly run the flashed binary

    // TODO collect trace until some stop condition

    // TODO save trace somewhere for offline analysis.

    // println!("Hello, world!");

    // Note: the first argument is the name of this binary
    // (`cargo-rtic-trace`), which we do not care about.
}
