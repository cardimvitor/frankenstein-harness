#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(target_os = "linux")]
    if args.first().map(|a| a == "__sandbox-exec").unwrap_or(false) {
        // hidden: confine this process with Landlock + seccomp, then exec the command (see util::landlock)
        std::process::exit(fh::util::landlock::sandbox_exec_main(&args[1..]));
    }
    let code = fh::ui::cli::main(args).await;
    std::process::exit(code);
}
