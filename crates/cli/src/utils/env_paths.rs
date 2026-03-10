use std::collections::HashMap;
use std::path::PathBuf;

fn env_path(env: Option<&HashMap<String, String>>, key: &str) -> Option<PathBuf> {
    env.and_then(|vars| vars.get(key))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub(crate) fn home_dir_from_env(env: Option<&HashMap<String, String>>) -> Option<PathBuf> {
    env_path(env, "HOME")
        .or_else(|| env_path(env, "USERPROFILE"))
        .or_else(|| {
            let drive = env
                .and_then(|vars| vars.get("HOMEDRIVE"))
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())?;
            let path = env
                .and_then(|vars| vars.get("HOMEPATH"))
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())?;
            Some(PathBuf::from(format!("{drive}{path}")))
        })
        .or_else(dirs::home_dir)
}

pub(crate) fn data_dir_from_env(env: Option<&HashMap<String, String>>) -> Option<PathBuf> {
    env_path(env, "XDG_DATA_HOME")
        .or_else(|| env_path(env, "LOCALAPPDATA"))
        .or_else(|| env_path(env, "APPDATA"))
        .or_else(|| home_dir_from_env(env).map(|home| home.join(".local").join("share")))
        .or_else(dirs::data_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_dir_from_env_prefers_home_override() {
        let env = HashMap::from([("HOME".to_string(), "/tmp/custom-home".to_string())]);

        assert_eq!(
            home_dir_from_env(Some(&env)),
            Some(PathBuf::from("/tmp/custom-home"))
        );
    }
}
