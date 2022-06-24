use std::{
    fs,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    str::from_utf8,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use duct::cmd;

#[derive(Debug)]
struct HostGroup {
    name: &'static str,
    group: &'static str,
    hosts: &'static [&'static str],
    config: &'static str,
    password: &'static str,
    controller: &'static str,
}

static HOSTS: &[HostGroup] = &[
    HostGroup {
        name: "mcoms",
        group: "mcoms",
        hosts: &["mcom1", "mcom2", "mcom3", "mcom4"],
        config: "mcom",
        password: "root",
        controller: "railchain-pi",
    },
    HostGroup {
        name: "rpis",
        group: "rpis",
        hosts: &["pi1", "pi2", "pi3", "pi4"],
        config: "pi",
        password: "rootroot",
        controller: "railchain-pi",
    },
    HostGroup {
        name: "mcoms-aws",
        group: "mcoms",
        hosts: &["mcom1", "mcom2", "mcom3", "mcom4"],
        config: "mcom",
        password: "root",
        controller: "aws-vm1",
    },
];

struct Program {
    name: &'static str,
    config: &'static str,
    stats: &'static str,
}

struct Series {
    name: &'static str,
    program: &'static str,
    argument: &'static str,
    values: &'static [&'static str],
}

static SERIES: &[Series] = &[
    Series {
        name: "mvb-payloads",
        program: "mvb-fm2",
        argument: "max_signals",
        values: &["5", "42", "85", "164"],
    },
    Series {
        name: "mvb-baseline-payloads",
        program: "mvb-baseline",
        argument: "max_signals",
        values: &["5", "42", "85", "164"],
    },
    Series {
        name: "sim-payloads",
        program: "sim-regular",
        argument: "payload_size",
        values: &["32", "256", "512", "1024", "2048", "4096", "6144", "8192"],
    },
    Series {
        name: "sim-tickrate",
        program: "sim-regular",
        argument: "interval",
        values: &["32", "48", "64", "96", "128", "192", "256"],
    },
    Series {
        name: "baseline-tickrate",
        program: "sim-baseline",
        argument: "interval",
        values: &["32", "48", "64", "96", "128", "192", "256"],
    },
    Series {
        name: "baseline-payloads",
        program: "sim-baseline",
        argument: "payload_size",
        values: &["32", "256", "512", "1024", "2048", "4096", "6144", "8192"],
    },
    Series {
        name: "sim-viewchanges",
        program: "sim-viewchange",
        argument: "vc_checkpoint_size",
        values: &["10", "20", "30", "40", "50", "60", "70", "80", "90", "100"],
    },
    Series {
        name: "export",
        program: "sim-export",
        argument: "num_blocks",
        values: &["500", "1000", "2000", "4000", "8000", "16000"],
        // values: &["4000"],
    },
    Series {
        name: "fm2-interval",
        program: "sim-fm2",
        argument: "interval",
        values: &["32", "48", "64", "96", "128", "192", "256"],
    },
    Series {
        name: "fm2-payloads",
        program: "sim-fm2",
        argument: "payload_size",
        values: &["32", "256", "512", "1024", "2048", "4096", "6144", "8192"],
    },
];

static PROGRAMS: &[Program] = &[
    Program {
        name: "mvb-regular",
        config: "",
        stats: "railchain",
    },
    Program {
        name: "mvb-baseline",
        config: "-pbft",
        stats: "baseline-server,baseline-client",
    },
    Program {
        name: "mvb-fm2",
        config: "-fm2",
        stats: "railchain",
    },
    Program {
        name: "sim-regular",
        config: "",
        stats: "railchain",
    },
    Program {
        name: "sim-baseline",
        config: "-pbft",
        stats: "baseline-server,baseline-client",
    },
    Program {
        name: "sim-viewchange",
        config: "",
        stats: "railchain",
    },
    Program {
        name: "sim-export",
        config: "-fm2",
        stats: "",
    },
    Program {
        name: "sim-export-aws",
        config: "-fm2",
        stats: "",
    },
    Program {
        name: "vc-demo",
        config: "",
        stats: "railchain",
    },
    Program {
        name: "fm2-viewchange",
        config: "-fm2",
        stats: "railchain",
    },
    Program {
        name: "baseline-viewchange",
        config: "-pbft",
        stats: "baseline-server",
    },
    Program {
        name: "sim-fm2",
        config: "-fm2",
        stats: "railchain",
    },
];

fn read_stats(host: &str, process: &str, mut file: &File, password: &str) -> anyhow::Result<()> {
    let output = cmd!(
        "sshpass",
        "-p",
        password,
        "ssh",
        host,
        format!("ps -o pid,rss,%cpu,cmd -C {}", process)
    )
    .stdout_capture()
    .stderr_capture()
    .run()?;

    if !output.status.success() {
        eprintln!("{}", from_utf8(&output.stderr).expect("utf8"));
    }
    file.write_all(&output.stdout)?;
    writeln!(file)?;

    Ok(())
}

fn run_program(
    Program { name, config, stats }: &'static Program,
    hosts: &HostGroup,
    duration: u64,
) -> anyhow::Result<()> {
    let date = chrono::offset::Utc::now();
    let dest: PathBuf = format!("./benchmarks/{}-{}/", name, date.to_rfc3339()).into();
    xshell::mkdir_p(&dest)?;
    let dest = dest.canonicalize().unwrap();

    let hostgroup = hosts.group;
    let config = format!("{}{}", hosts.config, config);

    xshell::cmd!("ansible-playbook  ./ansible/{name}.yaml --tags stop -e dest_dir={dest} -e h={hostgroup}").run()?;

    xshell::cmd!("ansible-playbook ./ansible/{name}.yaml --tags start -e h={hostgroup} -e bench_config={config}")
        .run()?;

    static RUNNING: AtomicBool = AtomicBool::new(true);

    for host in hosts.hosts {
        let path = dest.join(format!("stats-{}.txt", host));
        let pw = hosts.password;
        thread::spawn(move || {
            let file = File::create(path).expect("statsfile");
            while RUNNING.load(Ordering::Relaxed) {
                if let Err(e) = read_stats(host, stats, &file, pw) {
                    println!("stats: {}", e);
                }
                thread::sleep(Duration::from_secs(5));
            }
        });
    }

    thread::sleep(Duration::from_secs(10 + duration + 10));

    RUNNING.store(false, Ordering::Relaxed);

    xshell::cmd!("ansible-playbook  ./ansible/{name}.yaml --tags stop -e dest_dir={dest} -e h={hostgroup}").run()?;

    for file in xshell::read_dir(format!("config/{}/", config))? {
        xshell::cp(&file, dest.join(file.file_name().unwrap()))?;
    }

    Ok(())
}

fn run_series(
    Series { argument, values, .. }: &'static Series,
    Program { name, config, stats }: &'static Program,
    hosts: &HostGroup,
    duration: u64,
) -> anyhow::Result<()> {
    let date = chrono::offset::Utc::now();
    let dest: PathBuf = format!("./benchmarks/series-{}-{}-{}/", name, argument, date.to_rfc3339()).into();
    xshell::mkdir_p(&dest)?;
    let dest = dest.canonicalize().unwrap();

    let hostgroup = hosts.group;
    let config = format!("{}{}", hosts.config, config);

    xshell::cmd!("ansible-playbook  ./ansible/{name}.yaml --tags stop -e dest_dir={dest} -e h={hostgroup}").run()?;

    println!("Running series {} with {}: {:?}", name, argument, values);

    let spec = serde_json::json!({
        "argument": &argument,
        "values": &values,
    });
    fs::write(dest.join("spec.json"), &serde_json::to_vec_pretty(&spec)?)?;

    for value in values.iter() {
        println!("Running {}={}", argument, value);
        let dest = dest.join(format!("{}-{}", argument, value));
        xshell::mkdir_p(&dest)?;

        xshell::cmd!("ansible-playbook ./ansible/{name}.yaml --tags start -e {argument}={value} -e h={hostgroup} -e bench_config={config}").run()?;

        let running = Arc::new(AtomicBool::new(true));

        let mut threads = vec![];
        for host in hosts.hosts {
            let path = dest.join(format!("stats-{}.txt", host));
            let running = running.clone();
            let pw = hosts.password;
            let thread = thread::spawn(move || {
                let file = File::create(path).expect("statsfile");
                while running.load(Ordering::Relaxed) {
                    read_stats(host, stats, &file, pw).expect("stats");
                    thread::sleep(Duration::from_secs(5));
                }
            });
            threads.push(thread);
        }

        thread::sleep(Duration::from_secs(10 + duration + 10));

        running.store(false, Ordering::Relaxed);
        for thread in threads {
            let join = thread.join();
            if let Err(e) = join {
                println!("join: {:?}", e);
            }
        }

        {
            // let _tries = 0;
            xshell::cmd!("ansible-playbook ./ansible/{name}.yaml --tags stop -e dest_dir={dest} -e h={hostgroup}")
                .run()?;

            // match assert_logs("railchain", &dest) {
            //     Ok(_) => (),
            //     Err(_) => {
            //         println!("missing logs -- re-sync");
            //         xshell::cmd!("ansible-playbook ./ansible/{name}.yaml --tags stop -e dest_dir={dest}").run()?;
            //         assert_logs("railchain", &dest).expect("missing logs");
            //     }
            // }
        }

        for file in xshell::read_dir(format!("config/{}/", config))? {
            xshell::cp(&file, dest.join(file.file_name().unwrap()))?;
        }

        println!("Finished {}={}", argument, value);
        thread::sleep(Duration::from_secs(30));
    }

    Ok(())
}

fn run_export(
    Series { argument, values, .. }: &'static Series,
    Program { name, config, .. }: &'static Program,
    hosts: &HostGroup,
) -> anyhow::Result<()> {
    let date = chrono::offset::Utc::now();
    let dest: PathBuf = format!("./benchmarks/series-{}-{}-{}/", "export", argument, date.to_rfc3339()).into();
    xshell::mkdir_p(&dest)?;
    let dest = dest.canonicalize().unwrap();

    let hostgroup = hosts.group;
    let config = format!("{}{}", hosts.config, config);

    println!("Running series export with {}: {:?}", argument, values);

    let spec = serde_json::json!({
        "argument": &argument,
        "values": &values,
    });
    fs::write(dest.join("spec.json"), &serde_json::to_vec_pretty(&spec)?)?;

    println!("Running export export with {}: {:?}", argument, values);

    let controller = hosts.controller;

    for value in values.iter() {
        let runs = 5;
        let dest = dest.join(format!("{}-{}", argument, value));

        xshell::cmd!("ansible-playbook ./ansible/{name}.yaml -e {argument}={value} -e h={hostgroup} -e eh={controller} -e bench_config={config} --tags generate").run()?;

        for i in 0..runs {
            let dest = dest.join(i.to_string());
            xshell::mkdir_p(&dest).expect("dest");

            xshell::cmd!("ansible-playbook ./ansible/{name}.yaml -e {argument}={value} -e h={hostgroup} -e bench_config={config} -e dest_dir={dest} -e eh={controller} --tags start,stop").run()?;
        }
    }

    Ok(())
}

fn run_single_viewchange(Program { name, config, stats }: &'static Program, hosts: &HostGroup) -> anyhow::Result<()> {
    let date = chrono::offset::Utc::now();
    let dest: PathBuf = format!("./benchmarks/{}-{}/", name, date.to_rfc3339()).into();
    xshell::mkdir_p(&dest)?;
    let dest = dest.canonicalize().unwrap();

    let hostgroup = hosts.group;
    let config = format!("{}{}", hosts.config, config);

    xshell::cmd!("ansible-playbook ./ansible/{name}.yaml --tags start -e payload_size=1024 -e interval=64 -e h={hostgroup} -e bench_config={config} -e offset=710 -e vc_freq=999999999 -e vc_checkpoint_size=10").run()?;

    static RUNNING: AtomicBool = AtomicBool::new(true);

    for host in hosts.hosts {
        let path = dest.join(format!("stats-{}.txt", host));
        let pw = hosts.password;
        thread::spawn(move || {
            let file = File::create(path).expect("statsfile");
            while RUNNING.load(Ordering::Relaxed) {
                if let Err(e) = read_stats(host, stats, &file, pw) {
                    println!("stats: {}", e);
                }
                thread::sleep(Duration::from_secs(5));
            }
        });
    }

    thread::sleep(Duration::from_secs(60));

    RUNNING.store(false, Ordering::Relaxed);

    xshell::cmd!("ansible-playbook  ./ansible/{name}.yaml --tags stop -e dest_dir={dest} -e h={hostgroup}").run()?;

    for file in xshell::read_dir(format!("config/{}/", config))? {
        xshell::cp(&file, dest.join(file.file_name().unwrap()))?;
    }

    Ok(())
}

#[allow(unused)]
fn assert_logs(prefix: &str, dest: &Path) -> anyhow::Result<()> {
    for i in 1..=4 {
        let filename = format!("{}-mcom{}.log", prefix, i);
        if !dest.join(&filename).is_file() {
            anyhow::bail!("{} does not exist", filename);
        }
    }
    Ok(())
}

pub(crate) fn run(
    super::Bench {
        program,
        series,
        duration,
        upload,
        hosts,
    }: super::Bench,
) -> anyhow::Result<()> {
    println!("running in {}", xshell::cwd()?.display());

    let hosts = HOSTS.iter().find(|h| h.name == hosts).unwrap_or(&HOSTS[0]);
    print!("hosts: {:?}", hosts);

    if let Some(series) = series {
        let series = SERIES.iter().find(|s| s.name == series).expect("series");
        let program = PROGRAMS.iter().find(|p| p.name == series.program).expect("program");

        if upload {
            let name = program.name;
            let hostgroup = hosts.group;
            let config = format!("{}{}", hosts.config, program.config);
            let controller = hosts.controller;
            xshell::cmd!(
                "ansible-playbook ./ansible/{name}.yaml -e h={hostgroup} -e eh={controller} --tags upload -e bench_config={config}"
            )
            .run()?;
        }
        if series.name == "export" {
            run_export(series, program, hosts)?;
        } else {
            run_series(series, program, hosts, duration)?;
        }
    } else if let Some(program) = program {
        let program_object = PROGRAMS.iter().find(|p| p.name == program).expect("program");
        if program_object.name == "vc-demo" {
            let program_object = PROGRAMS.iter().find(|p| p.name == "sim-viewchange").expect("program");
            if upload {
                let hostgroup = hosts.group;
                let config = format!("{}{}", hosts.config, program_object.config);
                let program = program_object.name;
                xshell::cmd!(
                    "ansible-playbook ./ansible/{program}.yaml -e h={hostgroup} --tags upload -e bench_config={config}"
                )
                .run()?;
            }
            run_single_viewchange(program_object, hosts)?;
        } else if program_object.name == "baseline-viewchange" || program_object.name == "fm2-viewchange" {
            if upload {
                let hostgroup = hosts.group;
                let config = format!("{}{}", hosts.config, program_object.config);
                let program = program_object.name;
                xshell::cmd!(
                    "ansible-playbook ./ansible/{program}.yaml -e h={hostgroup} --tags upload -e bench_config={config}"
                )
                .run()?;
            }
            run_single_viewchange(program_object, hosts)?;
        } else {
            if upload {
                let hostgroup = hosts.group;
                let config = format!("{}{}", hosts.config, program_object.config);
                xshell::cmd!(
                    "ansible-playbook ./ansible/{program}.yaml -e h={hostgroup} --tags upload -e bench_config={config}"
                )
                .run()?;
            }
            run_program(program_object, hosts, duration)?;
        }
    } else {
        println!("Must specify --series or --program");
    }

    Ok(())
}
