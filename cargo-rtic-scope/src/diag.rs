pub trait DiagnosableError: std::error::Error {
    fn diagnose(&self) -> Vec<String> {
        vec![]
    }
}
