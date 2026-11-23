use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Step banner
    Step,
    Info,
    Ok,
    Warn,
    Error,
    /// Raw output from a Papyrus tool
    Tool,
}

/// Cheap, cloneable log sink shared by the pipeline steps.
#[derive(Clone)]
pub struct Logger {
    sink: Arc<dyn Fn(Level, String) + Send + Sync>,
}

impl Logger {
    pub fn new(f: impl Fn(Level, String) + Send + Sync + 'static) -> Self {
        Self { sink: Arc::new(f) }
    }

    pub fn log(&self, level: Level, msg: impl Into<String>) {
        (self.sink)(level, msg.into())
    }

    pub fn step(&self, m: impl Into<String>) {
        self.log(Level::Step, m)
    }
    pub fn info(&self, m: impl Into<String>) {
        self.log(Level::Info, m)
    }
    pub fn ok(&self, m: impl Into<String>) {
        self.log(Level::Ok, m)
    }
    pub fn warn(&self, m: impl Into<String>) {
        self.log(Level::Warn, m)
    }
    pub fn error(&self, m: impl Into<String>) {
        self.log(Level::Error, m)
    }
    pub fn tool(&self, m: impl Into<String>) {
        self.log(Level::Tool, m)
    }
}
