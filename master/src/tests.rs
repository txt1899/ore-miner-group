use crate::lua::LuaScript;

#[test]
fn lua_test() {
    let code = r#"
function dynamic_gas(difficulty, gas_lamports)
    return (difficulty - 10) * 1000 + gas_lamports;
end

function dynamic_tip(difficulty, tip_lamports)
    return math.ceil((1 + (difficulty - 10) / 100) * tip_lamports);
end

"#;
    let script = LuaScript::new();
    script.load_script(code).unwrap();
    assert_eq!(script.new_gas(25, 10000).unwrap(), 25000);
    assert_eq!(script.new_jito_tip(23, 20000).unwrap(), 22600);
}
