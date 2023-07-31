use std::env::current_dir;
use std::process::exit;
use clap::Parser;
use key_value_db::{Result, KvsError, KvStore};

fn main() -> Result<()> {

    let opts: Opts = Opts::parse();

    let mut store = KvStore::open(current_dir()?)?;

    match opts.commond {

        Command::Get(get) => {
            let value = store.get(get.key)?;
            match value {
                Some(value) => {
                    println!("{}", value)
                },
                None => {
                    println!("Key not found")
                }
            }
        }
        Command::Set(set) => {
            store.set(set.key, set.value)?;
        }
        Command::Rm(rm) => {
            match store.remove(rm.key) {
                Ok(()) => {},
                Err(KvsError::KeyNotFound) => {
                    println!("Key not found");
                    exit(1);
                },
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}

#[derive(Parser,Debug)]
struct Opts {
    #[clap(subcommand)]
    commond: Command
}

#[derive(Parser,Debug)]
pub enum Command {
    #[clap()]
    Set(Set),
    #[clap()]
    Get(Get),
    #[clap()]
    Rm(Rm)
}

#[derive(Parser,Debug)]
pub struct Set {
    #[clap()]
    key:String,
    #[clap()]
    value:String,
}

#[derive(Parser, Debug)]
pub struct Get {
    #[clap()]
    key: String
}


#[derive(Parser, Debug)]
pub struct Rm {
    #[clap()]
    key: String
}