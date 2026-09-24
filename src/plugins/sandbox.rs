use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mlua::chunk::ChunkMode;
use mlua::{HookTriggers, Lua, LuaOptions, StdLib, Table, VmState};

const OS_REMOVED: [&str; 7] = [
    "execute",
    "exit",
    "remove",
    "rename",
    "tmpname",
    "getenv",
    "setlocale",
];
const GLOBALS_REMOVED: [&str; 5] = ["load", "loadstring", "loadfile", "dofile", "require"];
const HOOK_EVERY: u32 = 10_000;

/// The end of the current Lua call; `None` between calls.
pub type Deadline = Rc<Cell<Option<Instant>>>;

pub fn remaining(deadline: &Deadline) -> Option<Duration> {
    deadline
        .get()
        .map(|d| d.saturating_duration_since(Instant::now()))
}

/// A Lua state with only string, table, math, utf8 and a trimmed os library,
/// a memory cap and an instruction hook that enforces the call deadline.
pub struct Sandbox {
    pub lua: Lua,
    pub deadline: Deadline,
}

impl Sandbox {
    pub fn new(memory_limit: usize, stop: Arc<AtomicBool>) -> mlua::Result<Self> {
        let libs = StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::OS | StdLib::UTF8;
        let lua = Lua::new_with(libs, LuaOptions::new())?;
        let globals = lua.globals();
        let os: Table = globals.get("os")?;
        for name in OS_REMOVED {
            os.set(name, mlua::Nil)?;
        }
        for name in GLOBALS_REMOVED {
            globals.set(name, mlua::Nil)?;
        }
        let deadline: Deadline = Rc::new(Cell::new(None));
        let hook_deadline = deadline.clone();
        lua.set_hook(
            HookTriggers::new().every_nth_instruction(HOOK_EVERY),
            move |_, _| {
                if stop.load(Ordering::Relaxed) {
                    return Err(mlua::Error::runtime("stopped"));
                }
                match hook_deadline.get() {
                    Some(end) if Instant::now() >= end => {
                        Err(mlua::Error::runtime("time limit exceeded"))
                    }
                    _ => Ok(VmState::Continue),
                }
            },
        )?;
        lua.set_memory_limit(memory_limit)?;
        Ok(Self { lua, deadline })
    }

    pub fn arm(&self, timeout: Duration) {
        self.deadline.set(Some(Instant::now() + timeout));
    }

    pub fn disarm(&self) {
        self.deadline.set(None);
    }

    /// Runs a plugin file and returns the table it evaluates to. Text chunks
    /// only: Lua does not verify bytecode, so a binary chunk could break out.
    pub fn load_file(&self, path: &Path, source: &str) -> mlua::Result<Table> {
        let source = source.strip_prefix('\u{feff}').unwrap_or(source);
        let name = path
            .file_name()
            .map_or_else(|| "plugin".into(), |n| n.to_string_lossy().into_owned());
        self.lua
            .load(source)
            .set_name(format!("@{name}"))
            .set_mode(ChunkMode::Text)
            .eval::<Table>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox() -> Sandbox {
        Sandbox::new(8 << 20, Arc::new(AtomicBool::new(false))).unwrap()
    }

    #[test]
    fn dangerous_functions_are_gone() {
        let sb = sandbox();
        let check: bool = sb
            .lua
            .load(
                "return io == nil and require == nil and load == nil and loadfile == nil \
                 and dofile == nil and os.execute == nil and os.getenv == nil \
                 and os.remove == nil and os.time ~= nil and string.format ~= nil",
            )
            .eval()
            .unwrap();
        assert!(check);
    }

    #[test]
    fn bytecode_is_rejected_and_bom_is_stripped() {
        let sb = sandbox();
        let err = sb
            .load_file(Path::new("x.lua"), "\x1bLua\x54\x00junk")
            .unwrap_err();
        assert!(err.to_string().contains("binary"), "{err}");
        let t = sb
            .load_file(Path::new("x.lua"), "\u{feff}return { a = 1 }")
            .unwrap();
        assert_eq!(t.get::<i64>("a").unwrap(), 1);
    }

    #[test]
    fn deadline_stops_an_endless_loop() {
        let sb = sandbox();
        sb.arm(Duration::from_millis(100));
        let start = Instant::now();
        let err = sb.lua.load("while true do end").exec().unwrap_err();
        assert!(err.to_string().contains("time limit exceeded"), "{err}");
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn memory_limit_holds() {
        let sb = Sandbox::new(1 << 20, Arc::new(AtomicBool::new(false))).unwrap();
        let err = sb
            .lua
            .load("local t = {} for i = 1, 1e7 do t[i] = string.rep('x', 64) .. i end")
            .exec()
            .unwrap_err();
        assert!(matches!(err, mlua::Error::MemoryError(_)), "{err}");
    }
}
