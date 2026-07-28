use std::{fs, path::PathBuf};

use clap::{Args, Subcommand};

use super::{LocalPhiHome, PhiHome, SqlitePhiHome};

#[derive(Args)]
pub struct HomeArgs {
    #[command(subcommand)]
    pub command: HomeCommand,
}

#[derive(Subcommand)]
pub enum HomeCommand {
    #[command(
        about = "Create a local .phi home directory at PATH",
        before_help = crate::banner::startup_banner()
    )]
    New(HomeNewArgs),
    #[command(
        about = "Pack a local .phi home directory into a sqlite home file",
        before_help = crate::banner::startup_banner()
    )]
    Pack(HomePackArgs),
    #[command(
        about = "Unpack a sqlite home file into a local .phi home directory",
        before_help = crate::banner::startup_banner()
    )]
    Unpack(HomeUnpackArgs),
}

#[derive(Args)]
pub struct HomeNewArgs {
    pub path: PathBuf,
}

#[derive(Args)]
pub struct HomePackArgs {
    pub path: PathBuf,
    #[arg(short = 'o', long = "output")]
    pub output: Option<PathBuf>,
}

#[derive(Args)]
pub struct HomeUnpackArgs {
    pub path: PathBuf,
    #[arg(short = 'o', long = "output")]
    pub output: Option<PathBuf>,
}

pub fn run(args: HomeArgs) -> Result<(), Box<dyn std::error::Error>> {
    match args.command {
        HomeCommand::New(args) => create_local_home(args.path),
        HomeCommand::Pack(args) => pack_local_home(args.path, args.output),
        HomeCommand::Unpack(args) => unpack_sqlite_home(args.path, args.output),
    }
}

fn create_local_home(path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(&path)?;
    fs::create_dir_all(path.join("plugins"))?;
    fs::create_dir_all(path.join("templates"))?;
    fs::create_dir_all(path.join("tmp"))?;
    let config_path = path.join("config.toml");
    if !config_path.exists() {
        fs::write(config_path, b"")?;
    }
    println!("created local phi home at {}", path.display());
    Ok(())
}

fn pack_local_home(
    path: PathBuf,
    output: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    if !path.is_dir() {
        return Err(format!("phi home path is not a directory: {}", path.display()).into());
    }

    let home = LocalPhiHome::from_root(path.clone())?;
    let output = output.unwrap_or_else(|| path.with_extension("phihome"));
    let entries = home.entries()?;
    SqlitePhiHome::from_entries(output.clone(), &entries)?;
    println!(
        "packed local phi home {} -> {}",
        path.display(),
        output.display()
    );
    Ok(())
}

fn unpack_sqlite_home(
    path: PathBuf,
    output: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    if !path.exists() {
        return Err(format!("phi home sqlite file does not exist: {}", path.display()).into());
    }
    if path.is_dir() {
        return Err(format!("phi home sqlite input is a directory: {}", path.display()).into());
    }

    let home = SqlitePhiHome::from_path(path.clone())?;
    let output = output.unwrap_or_else(|| path.with_extension("phi"));
    let entries = home.entries()?;
    LocalPhiHome::from_entries(output.clone(), &entries)?;
    println!(
        "unpacked sqlite phi home {} -> {}",
        path.display(),
        output.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{HomeArgs, HomeCommand, HomeNewArgs, HomePackArgs, HomeUnpackArgs, run};
    use crate::home::{LocalPhiHome, PhiHome};

    #[test]
    fn new_creates_expected_local_home_layout() {
        let path = unique_temp_path("phi-home-new");
        run(HomeArgs {
            command: HomeCommand::New(HomeNewArgs { path: path.clone() }),
        })
        .expect("home new should succeed");

        assert!(path.is_dir());
        assert!(path.join("config.toml").is_file());
        assert!(path.join("plugins").is_dir());
        assert!(path.join("templates").is_dir());
        assert!(path.join("tmp").is_dir());

        std::fs::remove_dir_all(path).expect("temp home should be removable");
    }

    #[test]
    fn pack_and_unpack_round_trip_home_entries() {
        let local_path = unique_temp_path("phi-home-pack-local");
        let packed_path = unique_temp_path("phi-home-pack-packed").with_extension("phihome");
        let unpacked_path = unique_temp_path("phi-home-pack-unpacked");

        run(HomeArgs {
            command: HomeCommand::New(HomeNewArgs {
                path: local_path.clone(),
            }),
        })
        .expect("home new should succeed");
        std::fs::write(
            local_path.join("config.toml"),
            "PHI_MODEL = \"demo\"\nPHI_SYSTEM = \"hi\"\n",
        )
        .expect("config should be writable");
        std::fs::write(local_path.join("plugins").join("hello.py"), "print('hi')\n")
            .expect("plugin should be writable");
        std::fs::write(
            local_path.join("templates").join("hello.html"),
            "<user>hello</user>\n",
        )
        .expect("template should be writable");

        run(HomeArgs {
            command: HomeCommand::Pack(HomePackArgs {
                path: local_path.clone(),
                output: Some(packed_path.clone()),
            }),
        })
        .expect("home pack should succeed");
        run(HomeArgs {
            command: HomeCommand::Unpack(HomeUnpackArgs {
                path: packed_path.clone(),
                output: Some(unpacked_path.clone()),
            }),
        })
        .expect("home unpack should succeed");

        let local_entries = LocalPhiHome::from_root(local_path.clone())
            .expect("local home should load")
            .entries()
            .expect("local entries should load");
        let unpacked_entries = LocalPhiHome::from_root(unpacked_path.clone())
            .expect("unpacked local home should load")
            .entries()
            .expect("unpacked entries should load");
        assert_eq!(local_entries, unpacked_entries);

        std::fs::remove_dir_all(local_path).expect("temp local home should be removable");
        std::fs::remove_dir_all(unpacked_path).expect("temp unpacked home should be removable");
        std::fs::remove_file(packed_path).expect("temp packed home should be removable");
    }

    fn unique_temp_path(prefix: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nanos}"))
    }
}
