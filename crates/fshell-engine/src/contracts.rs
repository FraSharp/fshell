// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

/// Preferred representation of an external producer's stdout. Native typed
/// producers retain their records; this preference never stringifies them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputPreference {
    #[default]
    Records,
    Bytes,
}

/// Registry metadata, not parser keyword knowledge. Re-registering a handler
/// resets its contract; aliases, functions, and disabled builtins do not inherit
/// the contract of a builtin they shadow.
#[derive(Clone, Copy, Debug, Default)]
pub struct BuiltinContract {
    pub input: InputPreference,
    pub query: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Env;
    use std::sync::Arc;

    #[test]
    fn contracts_do_not_survive_handler_replacement_or_shadowing() {
        let env = Env::for_command();
        let handler: crate::BuiltinHandler = Arc::new(|_, _, _, _, _| Ok(()));
        let contract = BuiltinContract {
            input: InputPreference::Bytes,
            query: true,
        };
        env.register_builtin_with_contract("search", handler.clone(), contract);
        assert!(env.builtin_contract("search").query);
        env.register_builtin("search", handler.clone());
        assert!(!env.builtin_contract("search").query);
        env.register_builtin_with_contract("search", handler.clone(), contract);
        env.register_alias("search", "echo override");
        assert_eq!(
            env.builtin_contract("search").input,
            InputPreference::Records
        );
        env.aliases.write().clear();
        env.options.write().disabled_builtins.push("search".into());
        assert!(!env.builtin_contract("search").query);
        env.options.write().disabled_builtins.clear();
        env.fns
            .write()
            .insert("search".into(), (vec![], None, vec![]));
        assert!(!env.builtin_contract("search").query);
    }
}
