pub fn login_hint() -> String {
    format!("npx codemod@{} login", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn never_latest() {
        assert!(!super::login_hint().contains("codemod@latest"));
    }
}
