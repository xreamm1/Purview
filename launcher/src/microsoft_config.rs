//! Public OAuth application configuration. Account credentials stay in account::Store.
use std::path::Path;

pub const INVALID_ID: &str =
    "Copy a valid Application (client) ID from your Microsoft app registration.";
const FILE: &str = "microsoft-application.txt";

fn normalize(value: &str) -> Option<String> {
    let value = value.trim();
    (value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        }))
    .then(|| value.to_ascii_lowercase())
}

pub fn load_client_id() -> Option<String> {
    load_at(&crate::backend::data_dir())
}

fn load_at(dir: &Path) -> Option<String> {
    normalize(&std::fs::read_to_string(dir.join(FILE)).ok()?)
}

pub fn save_client_id(value: &str) -> Result<(), String> {
    save_at(&crate::backend::data_dir(), value)
}

fn save_at(dir: &Path, value: &str) -> Result<(), String> {
    let id = normalize(value).ok_or(INVALID_ID)?;
    static WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = WRITE
        .lock()
        .map_err(|_| "Microsoft application configuration is busy")?;
    let path = dir.join(FILE);
    let temp = dir.join(format!("microsoft-application.{}.tmp", std::process::id()));
    let result = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&temp, id))
        .and_then(|()| std::fs::rename(&temp, &path));
    if let Err(error) = result {
        let _ = std::fs::remove_file(temp);
        return Err(format!("{}: {error}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_configuration_roundtrips_and_invalid_input_keeps_it() -> std::io::Result<()> {
        let dir = std::env::temp_dir().join(format!("purview-oauth-{}", std::process::id()));
        let id = "01234567-89AB-CDEF-0123-456789ABCDEF";
        assert!(save_at(&dir, &format!(" {id} \n")).is_ok());
        assert_eq!(
            load_at(&dir).as_deref(),
            Some("01234567-89ab-cdef-0123-456789abcdef")
        );
        assert!(save_at(&dir, "not an application id").is_err());
        assert_eq!(
            load_at(&dir).as_deref(),
            Some("01234567-89ab-cdef-0123-456789abcdef")
        );
        std::fs::remove_file(dir.join(FILE))?;
        std::fs::remove_dir(dir)
    }
}
