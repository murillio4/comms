use windows::core::s;
use windows::core::PCSTR;
use windows::Win32::System::Registry;

pub fn run() {
    let mut key = Registry::HKEY::default();
    let mut disposition = Registry::REG_CREATE_KEY_DISPOSITION::default();

    unsafe {
        Registry::RegCreateKeyExA(
            Registry::HKEY_CURRENT_USER,
            s!("SOFTWARE\\Classes\\comms"),
            0u32,
            PCSTR::null(),
            Registry::REG_OPTION_NON_VOLATILE,
            Registry::KEY_WRITE,
            None,
            &mut key,
            Some(&mut disposition),
        )
        .ok()
        .unwrap();
    }

    if disposition == Registry::REG_OPENED_EXISTING_KEY {
        return;
    }

    unsafe {
        Registry::RegSetValueA(
            key,
            PCSTR::null(),
            Registry::REG_SZ,
            Some(s!("URL:comms").as_bytes()),
        )
        .ok()
        .unwrap();

        Registry::RegSetKeyValueA(
            key,
            PCSTR::null(),
            s!("URL Protocol"),
            Registry::REG_SZ.0,
            None,
        )
        .ok()
        .unwrap();

        Registry::RegCreateKeyExA(
            key,
            s!("shell\\open\\command"),
            0u32,
            PCSTR::null(),
            Registry::REG_OPTION_NON_VOLATILE,
            Registry::KEY_WRITE,
            None,
            &mut key,
            None,
        )
        .ok()
        .unwrap();

        // ".replace()" is hacky way to remove the "\\?\" prefix that the "shell\open\command"
        // configuration doesn't understand.
        let comms_path = std::env::current_exe()
            .unwrap()
            .as_path()
            .to_owned()
            .to_string_lossy()
            .replace(r"\\?\", "");

        Registry::RegSetValueA(
            key,
            PCSTR::null(),
            Registry::REG_SZ,
            Some(PCSTR::from_raw(format!("\"{}\" \"%1\"\0", comms_path).as_ptr()).as_bytes()),
        )
        .ok()
        .unwrap();
    }
}
