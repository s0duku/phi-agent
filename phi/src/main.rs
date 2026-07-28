use std::process;

fn main() {
    if let Err(error) = phi::run() {
        if error.downcast_ref::<phi::ReportedCliError>().is_none() {
            eprintln!("{error}");
        }
        process::exit(1);
    }
}
