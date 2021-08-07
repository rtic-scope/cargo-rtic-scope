pub trait DiagnosableError {
    fn diagnose(&self) -> Vec<String> {
        vec![]
    }
}
