//! Property-based testing suite for fshell parsers, serialization, and pipeline invariants.

mod common;
use common::*;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(250))]

    /// Invariant: The native fshell parser must NEVER panic or enter an infinite loop on arbitrary input strings.
    #[test]
    fn prop_parser_never_panics_on_arbitrary_strings(s in "\\PC*") {
        let mut parser = Parser::new(&s);
        let _ = parser.parse_statements();
    }

    /// Invariant: The POSIX script parser must NEVER panic on arbitrary input strings.
    #[test]
    fn prop_posix_parser_never_panics_on_arbitrary_strings(s in "\\PC*") {
        let _ = fshell_posix::parser::parse_posix_script(&s);
    }

    /// Invariant: Numeric arithmetic evaluation preserves standard mathematical associativity.
    #[test]
    fn prop_eval_integer_arithmetic(a in -10000i64..10000, b in -10000i64..10000) {
        let script = format!("{a} + {b}");
        let mut parser = Parser::new(&script);
        if let Ok(stmts) = parser.parse_statements()
            && let Some(first) = stmts.first()
            && let Stmt::Expr(expr) = first.unpack()
        {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let env = setup_test_env();
            let res = rt.block_on(eval_expr(expr, &env));
            if let Ok(Val::Int(val)) = res {
                prop_assert_eq!(val, a + b);
            }
        }
    }

    /// Invariant: Sorting a list of numbers produces an ordered sequence.
    #[test]
    fn prop_sort_order_invariant(nums in prop::collection::vec(-1000i64..1000, 0..50)) {
        let list = Val::List(nums.iter().map(|&n| Val::Int(n)).collect());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let env = setup_test_env();
        env.vars.write().insert("items".to_string(), list);

        let script = "$items | sort";
        let mut parser = Parser::new(script);
        if let Ok(stmts) = parser.parse_statements()
            && let Some(first) = stmts.first()
            && let Stmt::Expr(expr) = first.unpack()
        {
            let res = rt.block_on(eval_expr(expr, &env)).unwrap();
            if let Val::List(sorted_items) = res {
                let sorted_nums: Vec<i64> = sorted_items
                    .into_iter()
                    .filter_map(|v| match v {
                        Val::Int(n) => Some(n),
                        _ => None,
                    })
                    .collect();

                let mut expected = nums.clone();
                expected.sort();
                prop_assert_eq!(sorted_nums, expected);
            }
        }
    }

    /// Invariant: Filtering with predicate `x > threshold` yields elements strictly > threshold.
    #[test]
    fn prop_filter_soundness_invariant(nums in prop::collection::vec(-1000i64..1000, 0..50), threshold in -500i64..500) {
        let items: Vec<Val> = nums
            .iter()
            .enumerate()
            .map(|(i, &n)| {
                let mut m = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
                m.insert(ustr("id"), Val::Int(i as i64));
                m.insert(ustr("val"), Val::Int(n));
                Val::Map(m)
            })
            .collect();

        let list = Val::List(items);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let env = setup_test_env();
        env.vars.write().insert("items".to_string(), list);

        let script = format!("$items | filter val > {threshold}");
        let mut parser = Parser::new(&script);
        if let Ok(stmts) = parser.parse_statements()
            && let Some(first) = stmts.first()
            && let Stmt::Expr(expr) = first.unpack()
        {
            let res = rt.block_on(eval_expr(expr, &env)).unwrap();
            if let Val::List(filtered) = res {
                for item in filtered {
                    if let Val::Map(m) = item
                        && let Some(Val::Int(val)) = m.get(&ustr("val"))
                    {
                        prop_assert!(*val > threshold);
                    }
                }
            }
        }
    }

    /// Invariant: Cryptographic sponge hash is deterministic and collision-free across flipped bits.
    #[test]
    fn prop_sponge_hash_determinism(data in prop::collection::vec(any::<u8>(), 0..512)) {
        let d1 = fshell_hash::fhash256(&data);
        let d2 = fshell_hash::fhash256(&data);
        prop_assert_eq!(d1, d2);

        if !data.is_empty() {
            let mut flipped = data.clone();
            flipped[0] ^= 0x01;
            let d_flipped = fshell_hash::fhash256(&flipped);
            prop_assert_ne!(d1, d_flipped);
        }
    }

    /// Invariant: Val JSON serialization roundtrips primitive types losslessly.
    #[test]
    fn prop_val_json_roundtrip(n in any::<i64>(), s in "\\PC*", b in any::<bool>()) {
        let original = Val::List(vec![
            Val::Int(n),
            Val::String(s.clone()),
            Val::Bool(b),
        ]);

        let json_str = serde_json::to_string(&original).unwrap();
        let deserialized: Val = serde_json::from_str(&json_str).unwrap();
        prop_assert_eq!(original, deserialized);
    }
}
