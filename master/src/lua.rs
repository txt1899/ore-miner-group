use mlua::prelude::*;
use std::{fs::File, io::Read};

pub struct LuaScript {
    lua: Lua,
}

impl LuaScript {
    pub fn new() -> LuaScript {
        LuaScript {
            lua: Lua::new(),
        }
    }

    pub fn load_from_file(&self, path: &str) -> Result<(), LuaError> {
        let mut file = File::open(path).expect("脚本文件不存在");
        let mut contents = String::new();
        file.read_to_string(&mut contents).expect("脚本文件读取失败");
        self.load_script(&contents)
    }

    pub fn load_script(&self, script: &str) -> Result<(), LuaError> {
        Ok(self.lua.load(script).exec()?)
    }

    pub fn new_gas(&self, difficulty: u64, gas_lamports: u64) -> Result<u64, LuaError> {
        let func: LuaFunction = self.lua.globals().get("dynamic_gas")?;
        Ok(func.call::<_, u64>((difficulty, gas_lamports))?)
    }

    pub fn new_jito_tip(&self, difficulty: u64, tip_lamports: u64) -> Result<u64, LuaError> {
        let func: LuaFunction = self.lua.globals().get("dynamic_tip")?;
        Ok(func.call::<_, u64>((difficulty, tip_lamports))?)
    }
}
