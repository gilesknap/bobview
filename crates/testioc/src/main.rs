use testioc::{Part, build};

const USAGE: &str = "usage: bobview-testioc [--only ea|motor|cam]...

Serves the example-services BL01T PVs over Channel Access and PV Access.
Honours EPICS_CA_SERVER_PORT and the other EPICS_CA(S)_* variables, and
EPICS_PVAS_SERVER_PORT / EPICS_PVAS_BROADCAST_PORT and the other
EPICS_PVA(S)_* variables.";

fn parse_args() -> Result<Vec<Part>, String> {
    let mut parts = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "--only" => {
                let v = args.next().ok_or("--only needs a value")?;
                let p = Part::parse(&v).ok_or(format!("unknown part {v:?}"))?;
                if !parts.contains(&p) {
                    parts.push(p);
                }
            }
            _ => return Err(format!("unexpected argument {arg:?}")),
        }
    }
    if parts.is_empty() {
        parts = Part::ALL.to_vec();
    }
    Ok(parts)
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let parts = match parse_args() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("bobview-testioc: {e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    let ioc = match build(&parts, None, None).await {
        Ok(ioc) => ioc,
        Err(e) => {
            eprintln!("bobview-testioc: startup failed: {e}");
            std::process::exit(1);
        }
    };
    let served: Vec<_> = parts.iter().map(|p| p.describe()).collect();
    let (pva_tcp, pva_udp) = ioc.pva_ports();
    println!(
        "bobview-testioc: CA port {}, serving {}",
        ioc.server.tcp_port(),
        served.join("; ")
    );
    println!("bobview-testioc: PVA served on TCP port {pva_tcp}, UDP search port {pva_udp}");
    if let Err(e) = ioc.run().await {
        eprintln!("bobview-testioc: {e}");
        std::process::exit(1);
    }
}
