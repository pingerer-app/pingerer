use pingerer::cli::{run, Io};
use pingerer::client::HttpTransport;
use std::io::IsTerminal;
use std::rc::Rc;

fn main() {
    let io = Io {
        out: Box::new(std::io::stdout()),
        err: Box::new(std::io::stderr()),
        out_tty: std::io::stdout().is_terminal(),
        input: Box::new(std::io::BufReader::new(std::io::stdin())),
        input_tty: std::io::stdin().is_terminal(),
        env: std::env::vars().collect(),
        transport: Rc::new(HttpTransport::default()),
        sleeper: Box::new(|seconds| std::thread::sleep(std::time::Duration::from_secs_f64(seconds))),
    };
    std::process::exit(run(std::env::args().skip(1).collect(), io));
}
