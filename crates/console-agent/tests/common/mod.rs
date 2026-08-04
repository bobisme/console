use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_PROJECT: AtomicUsize = AtomicUsize::new(0);

pub struct TestProject {
    root: PathBuf,
}

impl TestProject {
    pub fn new(tag: &str, title: &str, value: i64) -> Self {
        let sequence = NEXT_PROJECT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "console-source-project-{}-{sequence}-{tag}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("lua/game")).unwrap();
        let project = Self { root };
        project.set_title(title);
        project.write(
            "lua/main.lua",
            "local value = require('game.value')\n\
             function _init() project_value = value end\n\
             function _draw() cls(value) print('PROJECT '..value, 4, 4, 7) end\n",
        );
        project.set_value(value);
        project
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest(&self) -> PathBuf {
        self.root.join("console.toml")
    }

    pub fn set_title(&self, title: &str) {
        self.write(
            "console.toml",
            &format!(
                "manifest_version = 1\n\n[cart]\ntitle = {title:?}\nauthor = \"test\"\n\n[lua]\nentry = \"lua/main.lua\"\nroot = \"lua\"\n"
            ),
        );
    }

    pub fn set_value(&self, value: i64) {
        self.write("lua/game/value.lua", &format!("return {value}\n"));
    }

    #[allow(dead_code)]
    pub fn break_module(&self) {
        self.write("lua/game/value.lua", "return (\n");
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }
}

impl Drop for TestProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
