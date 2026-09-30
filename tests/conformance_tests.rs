//! Differential conformance corpus.
//!
//! This suite is the project's *compatibility contract*, expressed as data.
//! Each case runs in a deterministic fixture and is judged against a reference
//! shell (`bash --posix`, plus `dash` when installed) or, for fsh-native
//! behaviour no POSIX shell models, against a literal expectation.
//!
//! The corpus is organised by the boundary it exercises, ordered by dependency —
//! word model first, then composition, then the agent-facing dispatch path:
//!
//! * `conformance_word_model` — parsing → word representation → expansion → argv.
//! * `conformance_composition` — pipelines, `&&`/`||`/`;`, subshells, exit status.
//! * `conformance_redirection` — redirection ordering and fd duplication.
//! * `conformance_native_word_model` — the native engine's word model, reified literally.
//! * `conformance_dispatch` — default mode, which is what a coding agent actually gets.
//! * `conformance_real_world` — commands of the shape agents and build scripts emit.
//!
//! A case expected to fail today carries a strict `known_failure` marker naming
//! the exact engines that fail. If such a case starts passing, the suite fails
//! and demands the marker be removed — bugs are therefore both pinned down and
//! impossible to leave behind accidentally.
//!
//! Metrics note: this corpus is deliberately small and deliberate. A thousand
//! generated variants of `echo foo` would tell us less than these cases, which
//! are chosen to interact quoting, expansion, globbing and redirection.

mod common;

use common::conformance::{Case, Engine, assert_suite};

/// Parsing → word representation → expansion → argv, on the POSIX frontend.
///
/// This is the foundational boundary: if these semantics are wrong, everything
/// above them is wrong in the same way. `argvdump` isolates expansion from the
/// behaviour of any real utility.
fn word_model_cases() -> Vec<Case> {
    vec![
        // --- quoting forms -------------------------------------------------
        Case::posix_only("word/unquoted-literal", "argvdump abc").features(&["word", "unquoted"]),
        Case::posix_only("word/single-quoted-literal", "argvdump 'abc'")
            .features(&["word", "single_quote"]),
        Case::posix_only("word/double-quoted-literal", "argvdump \"abc\"")
            .features(&["word", "double_quote"]),
        Case::posix_only(
            "word/single-quoted-parameter-is-literal",
            "argvdump '$HOME'",
        )
        .features(&["word", "single_quote", "parameter"]),
        Case::posix_only("word/double-quoted-parameter-expands", "argvdump \"$HOME\"").features(&[
            "word",
            "double_quote",
            "parameter",
        ]),
        Case::posix_only("word/backslash-escaped-space", r"argvdump a\ b")
            .features(&["word", "backslash"]),
        Case::posix_only("word/single-quotes-contain-double", "argvdump 'say \"hi\"'")
            .features(&["word", "single_quote"]),
        Case::posix_only("word/double-quotes-contain-single", "argvdump \"it's\"")
            .features(&["word", "double_quote"]),
        Case::posix_only(
            "word/double-quote-escaped-quote",
            r#"argvdump "say \"hi\"""#,
        )
        .features(&["word", "double_quote", "backslash"]),
        Case::posix_only("word/double-quote-escaped-backslash", r#"argvdump "a\\b""#).features(&[
            "word",
            "double_quote",
            "backslash",
        ]),
        Case::posix_only("word/double-quote-escaped-dollar", r#"argvdump "a\$b""#).features(&[
            "word",
            "double_quote",
            "backslash",
        ]),
        Case::posix_only(
            "word/double-quote-preserves-backslash-n",
            r#"argvdump "a\nb""#,
        )
        .features(&["word", "double_quote", "backslash"]),
        Case::posix_only(
            "word/double-quote-preserves-backslash-t",
            r#"argvdump "a\tb""#,
        )
        .features(&["word", "double_quote", "backslash"]),
        Case::posix_only("word/double-quote-line-continuation", "argvdump \"a\\\nb\"").features(&[
            "word",
            "double_quote",
            "backslash",
        ]),
        // --- globbing and quote context ------------------------------------
        // Quoting must suppress pathname expansion: this is the invariant the
        // whole word model rests on.
        Case::posix_only("word/single-quoted-glob-is-literal", "argvdump '*.rs'").features(&[
            "word",
            "single_quote",
            "glob",
        ]),
        Case::posix_only("word/double-quoted-glob-is-literal", "argvdump \"*.rs\"").features(&[
            "word",
            "double_quote",
            "glob",
        ]),
        Case::posix_only("word/unquoted-glob-expands", "argvdump *.rs").features(&["word", "glob"]),
        Case::posix_only("word/quoted-parameter-then-glob", "argvdump \"$PWD\"/*.rs").features(&[
            "word",
            "glob",
            "parameter",
            "double_quote",
        ]),
        Case::posix_only("word/glob-with-quoted-tail", "argvdump a'*'.txt").features(&[
            "word",
            "glob",
            "single_quote",
        ]),
        Case::posix_only("word/no-match-glob-passes-through", "argvdump nomatch-*.zz")
            .features(&["word", "glob"]),
        // --- mixed quoted / unquoted words ---------------------------------
        Case::posix_only(
            "word/mixed-quoting-concatenates",
            "BAR='a b'; argvdump foo\"$BAR\"baz",
        )
        .features(&["word", "mixed", "parameter"]),
        Case::posix_only("word/mixed-quoted-literal-adjacent", "argvdump \"a\"\"b\"")
            .features(&["word", "mixed"]),
        // --- empty arguments ----------------------------------------------
        Case::posix_only(
            "word/empty-quoted-parameter-preserved",
            "argvdump \"$EMPTY\"",
        )
        .features(&["word", "empty", "double_quote"]),
        Case::posix_only("word/empty-unquoted-parameter-removed", "argvdump $EMPTY").features(&[
            "word",
            "empty",
            "field_split",
        ]),
        Case::posix_only("word/empty-literal-quoted-preserved", "argvdump \"\"").features(&[
            "word",
            "empty",
            "double_quote",
        ]),
        // --- command substitution ------------------------------------------
        Case::posix_only(
            "word/quoted-command-substitution-single-field",
            "argvdump \"$(printf 'a b')\"",
        )
        .features(&["word", "command_substitution", "double_quote"]),
        Case::posix_only(
            "word/unquoted-command-substitution-splits",
            "argvdump $(printf 'a b')",
        )
        .features(&["word", "command_substitution", "field_split"]),
        // --- arithmetic and tilde ------------------------------------------
        Case::posix_only("word/arithmetic-expansion", "argvdump $((1 + 2))")
            .features(&["word", "arithmetic"]),
        Case::posix_only("word/tilde-expands-to-home", "argvdump ~").features(&["word", "tilde"]),
        Case::posix_only("word/tilde-user-resolves", "argvdump ~root").features(&["word", "tilde"]),
        // --- field splitting ------------------------------------------------
        // Variables are assigned inside the script so these cases exercise
        // splitting rather than the separate inherited-environment defect.
        Case::posix_only("word/unquoted-parameter-splits", "BAR='a b'; argvdump $BAR")
            .features(&["word", "field_split"]),
        Case::posix_only(
            "word/quoted-parameter-does-not-split",
            "BAR='a b'; argvdump \"$BAR\"",
        )
        .features(&["word", "field_split", "double_quote"]),
        Case::posix_only(
            "word/ifs-colon-splits",
            "IFS=:; COLON='one:two:three'; argvdump $COLON",
        )
        .features(&["word", "field_split", "ifs"]),
        Case::posix_only(
            "word/mixed-quoted-prefix-still-splits",
            "BAR='a b'; argvdump \"x\"$BAR",
        )
        .features(&["word", "field_split", "mixed"])
        .known_failure(
            "posix-split-mixed-quoting",
            "field splitting is gated on the whole word; any quoting anywhere \
             suppresses splitting of the unquoted parts too, so `\"x\"$BAR` stays \
             one field instead of becoming `xa` and `b`",
            &[Engine::Posix],
        ),
        Case::posix_only(
            "word/mixed-quoted-suffix-still-splits",
            "BAR='a b'; argvdump $BAR\"x\"",
        )
        .features(&["word", "field_split", "mixed"])
        .known_failure(
            "posix-split-mixed-quoting",
            "field splitting is gated on the whole word; quoting anywhere \
             suppresses splitting of the unquoted parts too",
            &[Engine::Posix],
        ),
        // --- the inherited environment ---------------------------------------
        // A variable inherited from the process environment is readable, exactly
        // as it is in a POSIX shell; a variable fsh never learned about stays
        // unset. See `docs/POSIX-COMPATIBILITY-BASELINE.md` for the history.
        Case::posix_only("word/inherited-environment-is-visible", "argvdump \"$FOO\"").features(&[
            "word",
            "parameter",
            "environment",
        ]),
        Case::posix_only(
            "word/inherited-environment-visible-unquoted",
            "argvdump $FOO",
        )
        .features(&["word", "parameter", "environment"]),
        // The `${…}` modifier forms resolve through the same lookup, so they must
        // see inherited variables too.
        Case::posix_only(
            "word/inherited-environment-default-modifier",
            "argvdump \"${FOO:-fallback}\"",
        )
        .features(&["word", "parameter", "environment"]),
        Case::posix_only(
            "word/unset-environment-default-modifier",
            "argvdump \"${NOT_SET_ANYWHERE:-fallback}\"",
        )
        .features(&["word", "parameter", "environment"]),
        // `unset` must actually hide it, or the fallback would be un-unsettable.
        Case::posix_only(
            "word/unset-hides-inherited-environment",
            "unset FOO; argvdump \"${FOO:-gone}\"",
        )
        .features(&["word", "parameter", "environment"]),
        Case::posix_only(
            "word/inherited-environment-assignment-modifier",
            "argvdump \"${INHERITED_DEFAULT:=assigned}\" \"${INHERITED_DEFAULT}\"",
        )
        .features(&["word", "parameter", "environment"]),
        // Arithmetic expansion resolves names through its own path.
        Case::posix_only(
            "word/arithmetic-sees-inherited-environment",
            "argvdump \"$((NUMBER + 1))\"",
        )
        .features(&["word", "arithmetic", "environment"]),
        // --- special parameters ---------------------------------------------
        Case::posix_only(
            "word/at-quoted-yields-separate-fields",
            "set -- a b; argvdump \"$@\"",
        )
        .features(&["word", "special_parameter"]),
        Case::posix_only(
            "word/star-quoted-single-field",
            "set -- a b; argvdump \"$*\"",
        )
        .features(&["word", "special_parameter"]),
        Case::posix_only("word/star-unquoted-splits", "set -- a b; argvdump $*").features(&[
            "word",
            "special_parameter",
            "field_split",
        ]),
        Case::posix_only(
            "word/hash-counts-positionals",
            "set -- a b; argvdump \"$#\"",
        )
        .features(&["word", "special_parameter"]),
        Case::posix_only(
            "word/at-embedded-in-word-yields-per-field",
            "set -- a b; argvdump \"pre$@post\"",
        )
        .features(&["word", "special_parameter", "mixed"])
        .known_failure(
            "posix-dollar-at-embedded",
            "`$@` is joined with a space inside a larger word instead of producing \
             one field per positional parameter",
            &[Engine::Posix],
        ),
        // --- `name=value` as an ordinary argument ---------------------------
        // POSIX treats `name=value` as an assignment only *before* the command
        // word; after it the word is an ordinary argument whose shape is
        // irrelevant. Position comes from the parsed structure, not from spelling.
        Case::posix_only("word/assignment-looking-argument-kept", "argvdump n=1")
            .features(&["word", "assignment_argument"]),
        Case::posix_only("word/assignment-looking-arguments-kept", "argvdump a=b c=d")
            .features(&["word", "assignment_argument"]),
        Case::posix_only("word/long-flag-with-equals-kept", "argvdump --include=*.rs")
            .features(&["word", "assignment_argument"]),
        Case::posix_only(
            "word/printf-format-argument-with-equals",
            "printf '<%s>\\n' x=1",
        )
        .features(&["word", "assignment_argument"]),
        // The argument is still a word: it expands, splits and globs normally.
        Case::posix_only(
            "word/assignment-looking-argument-expands",
            "BAR='a b'; argvdump x=$BAR",
        )
        .features(&["word", "assignment_argument", "field_split"]),
        Case::posix_only(
            "word/assignment-looking-argument-expands-quoted",
            "BAR='a b'; argvdump x=\"$BAR\"",
        )
        .features(&["word", "assignment_argument", "double_quote"]),
        // A `name=value` argument holding a glob must not be swallowed either.
        Case::posix_only("word/assignment-looking-argument-globs", "argvdump f=*.rs").features(&[
            "word",
            "assignment_argument",
            "glob",
        ]),
        // --- temporary assignments before the command word still work -------
        Case::posix_only(
            "word/temporary-assignment-visible-to-child",
            "TMPV=bar argvdump \"$TMPV\"",
        )
        .features(&["word", "assignment"]),
        // A genuine prefix assignment does reach the child's environment, even
        // though it is invisible to that same command's word expansion.
        Case::posix_only(
            "word/prefix-assignment-reaches-child-environment",
            "TMPV=bar /usr/bin/printenv TMPV",
        )
        .features(&["word", "assignment"]),
        Case::posix_only(
            "word/suffix-assignment-does-not-set-variable",
            "argvdump n=1 > /dev/null; argvdump \"${n:-unset}\"",
        )
        .features(&["word", "assignment_argument", "assignment"]),
    ]
}

/// Pipelines, logical operators, subshells and exit-status propagation.
fn composition_cases() -> Vec<Case> {
    vec![
        Case::posix_only(
            "compose/semicolon-sequences",
            "emit --stdout a; emit --stdout b",
        )
        .features(&["compose", "sequence"]),
        Case::posix_only("compose/and-short-circuits", "false && emit --stdout nope")
            .features(&["compose", "and_if"]),
        Case::posix_only("compose/or-short-circuits", "true || emit --stdout nope")
            .features(&["compose", "or_if"]),
        Case::posix_only("compose/or-runs-on-failure", "false || emit --stdout yes")
            .features(&["compose", "or_if"]),
        Case::posix_only(
            "compose/exit-status-propagates",
            "emit --exit 7; emit --stdout \"rc=$?\"",
        )
        .features(&["compose", "exit_status"]),
        Case::posix_only(
            "compose/exit-status-after-failure",
            "false; emit --stdout \"rc=$?\"",
        )
        .features(&["compose", "exit_status"]),
        Case::posix_only(
            "compose/pipeline-passes-data",
            "printf 'b\\na\\n' | /usr/bin/sort",
        )
        .features(&["compose", "pipeline"]),
        Case::posix_only(
            "compose/pipeline-status-is-last-stage",
            "emit --exit 3 | emit --exit 7; emit --stdout \"rc=$?\"",
        )
        .features(&["compose", "pipeline", "exit_status"]),
        Case::posix_only(
            "compose/pipeline-status-ignores-earlier-failure",
            "emit --exit 3 | emit --exit 0; emit --stdout \"rc=$?\"",
        )
        .features(&["compose", "pipeline", "exit_status"]),
        // With pipefail, choose the rightmost failing stage, not the first one.
        Case::posix_only(
            "compose/pipefail-uses-rightmost-failure",
            "set -o pipefail; emit --exit 3 | emit --exit 7 | emit --exit 0; emit --stdout \"rc=$?\"",
        )
        .bash_only()
        .features(&["compose", "pipeline", "pipefail", "exit_status"]),
        Case::posix_only("compose/subshell-runs", "(emit --stdout sub)")
            .features(&["compose", "subshell"]),
        Case::posix_only(
            "compose/subshell-cd-isolated",
            "(cd dir && emit --stdout ok); emit --stdout \"pwd=$(basename \"$PWD\")\"",
        )
        .features(&["compose", "subshell"]),
        Case::posix_only(
            "compose/function-and-return-status",
            "f() { emit --stdout in; return 3; }; f; emit --stdout \"rc=$?\"",
        )
        .features(&["compose", "function"]),
        Case::posix_only(
            "compose/local-variable-in-function",
            "f() { local x=1; emit --stdout \"$x\"; }; f",
        )
        .compare_stderr()
        .features(&["compose", "function"]),
        // `local` declares a shell variable; POSIX shells scope it to the call it
        // was declared in. fsh declares it without a function-local scope, so the
        // name is still set once the function returns.
        Case::posix_only(
            "compose/local-does-not-leak-out-of-the-function",
            "f() { local x=1; }; f; argvdump \"${x-unset}\"",
        )
        .features(&["compose", "function", "scope"])
        .known_failure(
            "posix-local-has-no-function-scope",
            "`local` declares a shell variable but does not scope it to the call, so \
             the name is still set after the function returns",
            &[Engine::Posix],
        ),
        Case::posix_only(
            "compose/while-read-loop",
            "while read -r line; do emit --stdout \"[$line]\"; done < a.txt",
        )
        .features(&["compose", "loop"]),
        Case::posix_only(
            "compose/for-over-glob",
            "for f in *.rs; do emit --stdout \"$f\"; done",
        )
        .features(&["compose", "loop", "glob"]),
        Case::posix_only(
            "compose/case-matches-glob",
            "case a.rs in *.rs) emit --stdout rust;; esac",
        )
        .features(&["compose", "case"]),
        Case::posix_only(
            "compose/if-with-test",
            "if [ -f a.rs ]; then emit --stdout yes; else emit --stdout no; fi",
        )
        .features(&["compose", "if", "test"]),
    ]
}

/// Redirection wiring, duplication and ordering.
fn redirection_cases() -> Vec<Case> {
    vec![
        Case::posix_only(
            "redirect/stdout-to-file",
            "emit --stdout payload > out.txt",
        )
        .files(&["out.txt"])
        .features(&["redirect", "stdout"]),
        // A redirect target must be established before the command runs, whether
        // or not the command writes anything. `: > file` is the idiomatic way to
        // create or truncate a file, so a lazily-opened target breaks a very
        // common idiom (as well as `> build.log` on a silent command).
        Case::posix_only(
            "redirect/silent-command-still-creates-target",
            ": > created.txt; if [ -f created.txt ]; then emit --stdout exists; else emit --stdout missing; fi",
        )
        .files(&["created.txt"])
        .features(&["redirect", "stdout"]),
        Case::posix_only(
            "redirect/silent-command-still-truncates-target",
            "emit --stdout payload > truncated.txt; : > truncated.txt; if [ -s truncated.txt ]; then emit --stdout nonempty; else emit --stdout empty; fi",
        )
        .files(&["truncated.txt"])
        .features(&["redirect", "stdout"]),
        Case::posix_only(
            "redirect/append-to-file",
            "emit --stdout one > f.txt; emit --stdout two >> f.txt",
        )
        .files(&["f.txt"])
        .features(&["redirect", "append"]),
        Case::posix_only("redirect/stderr-to-file", "emit --stderr err 2> err.txt")
            .files(&["err.txt"])
            .compare_stderr()
            .features(&["redirect", "stderr"]),
        Case::posix_only(
            "redirect/stderr-to-devnull",
            "emit --stderr err 2> /dev/null",
        )
        .compare_stderr()
        .features(&["redirect", "stderr"]),
        Case::posix_only(
            "redirect/stdout-to-devnull",
            "emit --stdout out > /dev/null",
        )
        .features(&["redirect", "stdout"]),
        // `2>&1` must merge stderr into whatever stdout currently is.
        Case::posix_only(
            "redirect/stderr-merged-to-stdout",
            "emit --stdout o --stderr e 2>&1",
        )
        .compare_stderr()
        .features(&["redirect", "fd_dup"]),
        // Ordering matters: `> out 2>&1` sends both streams to the file...
        Case::posix_only(
            "redirect/stdout-then-merge-sends-both-to-file",
            "emit --stdout o --stderr e > both.txt 2>&1",
        )
        .files(&["both.txt"])
        .compare_stderr()
        .features(&["redirect", "fd_dup", "ordering"]),
        // ...while `2>&1 > out` merges first (keeping stderr on the terminal)
        // and only then redirects stdout. The two must stay distinguishable.
        Case::posix_only(
            "redirect/merge-then-stdout-keeps-stderr",
            "emit --stdout o --stderr e 2>&1 > both2.txt",
        )
        .files(&["both2.txt"])
        .compare_stderr()
        .features(&["redirect", "fd_dup", "ordering"]),
        Case::posix_only(
            "redirect/stdout-file-and-stderr-terminal",
            "emit --stdout o --stderr e 1> out3.txt",
        )
        .files(&["out3.txt"])
        .compare_stderr()
        .features(&["redirect", "stdout", "stderr"]),
        Case::posix_only(
            "redirect/stderr-merged-into-pipeline",
            "emit --stdout o --stderr e 2>&1 | /usr/bin/sort",
        )
        .compare_stderr()
        .features(&["redirect", "fd_dup", "pipeline"]),
        Case::posix_only(
            "redirect/stdout-merged-to-stderr",
            "emit --stdout o --stderr e 1>&2",
        )
        .compare_stderr()
        .features(&["redirect", "fd_dup"]),
        Case::posix_only(
            "redirect/merge-then-restore-to-stderr",
            "emit --stdout o --stderr e 1>&2 2>&1",
        )
        .compare_stderr()
        .features(&["redirect", "fd_dup", "ordering"]),
        Case::posix_only("redirect/close-stderr", "emit --stdout o --stderr e 2>&-")
            .compare_stderr()
            .features(&["redirect", "fd_dup", "closed"]),
        // `&>` is a bash extension, so bash alone is the oracle.
        Case::posix_only(
            "redirect/both-to-file",
            "emit --stdout o --stderr e &> both3.txt",
        )
        .bash_posix_only()
        .files(&["both3.txt"])
        .compare_stderr()
        .features(&["redirect", "fd_dup", "bash_extension"]),
        // A builtin's own diagnostics must follow the descriptors it was given,
        // not escape to the process's stderr. `> /dev/null 2>&1` sends both to
        // /dev/null, so a correct shell is silent on both streams. The trailing
        // `emit` pins the exit status, because dash and bash disagree about the
        // status of a bare failed `cd`.
        Case::posix_only(
            "redirect/builtin-diagnostic-follows-redirection",
            "cd /definitely-not-a-directory > /dev/null 2>&1; emit --stdout done",
        )
        .compare_stderr()
        .features(&["redirect", "fd_dup", "builtin"])
        .known_failure(
            "posix-builtin-diagnostics-bypass-redirection",
            "builtin diagnostics are written with bare `eprintln!`, so they ignore \
             the descriptor table entirely and still reach the process's stderr",
            &[Engine::Posix],
        ),
        // A compound command's own duplication is not yet propagated into the body
        // it evaluates, so `( … ) 2>&1` does not merge for the inner command.
        Case::posix_only("redirect/subshell-stderr-merged", "(emit --stderr e) 2>&1")
            .compare_stderr()
            .features(&["redirect", "fd_dup", "subshell"])
            .known_failure(
                "posix-compound-stderr-not-merged",
                "a compound command's redirects are applied to its own table but only \
                 stdout is forwarded to the body, so `( … ) 2>&1` leaves stderr alone",
                &[Engine::Posix],
            ),
        // Descriptors above 2 are rejected explicitly rather than silently ignored.
        // bash and dash both permit the form, so this is a declared capability gap
        // rather than undefined behaviour — the point is that it is never silent.
        Case::posix_only(
            "redirect/non-standard-descriptor",
            "emit --stdout o 3> fd3.txt",
        )
        .files(&["fd3.txt"])
        .features(&["redirect", "unsupported"])
        .known_failure(
            "posix-non-standard-fd-rejected",
            "redirection of fd 3+ is refused with an explicit error instead of being \
             silently dropped; a declared capability gap, reported as UnsupportedSyntax",
            &[Engine::Posix],
        ),
    ]
}

/// The native engine's word model, judged against literal expectations.
///
/// No reference shell can model fsh-native semantics, so the expectation is
/// written out. Quoting is the invariant under test: quoted text is literal, so
/// it must never glob.
fn native_word_model_cases() -> Vec<Case> {
    vec![
        // The native word model, stated as headline rules: a word is one typed
        // value and stays one, whether it came from a variable, a list or a
        // command substitution. `true` and `false` are Bool values here; the POSIX
        // engine owns them as utilities.
        Case::native(
            "native/unquoted-parameter-is-one-value",
            "BAR=\"a b\"; argvdump $BAR",
            "argc=1\narg[0]=\"a b\"\n",
            0,
        )
        .features(&["native", "word", "parameter"]),
        Case::native(
            "native/list-is-one-value",
            "let l = [1, 2]; argvdump $l",
            "argc=1\narg[0]=\"1\\n2\"\n",
            0,
        )
        .features(&["native", "word", "list"]),
        Case::native("native/bare-true-is-a-value", "true | cat", "true\n", 0)
            .features(&["native", "literal", "pipeline"]),
        Case::native("native/bare-false-is-a-value", "false | cat", "false\n", 0)
            .features(&["native", "literal", "pipeline"]),
        Case::native(
            "native/plain-word",
            "argvdump abc",
            "argc=1\narg[0]=\"abc\"\n",
            0,
        )
        .features(&["native", "word"]),
        Case::native(
            "native/double-quoted-parameter",
            "argvdump \"$FOO\"",
            "argc=1\narg[0]=\"hello\"\n",
            0,
        )
        .features(&["native", "word", "parameter"]),
        Case::native(
            "native/single-quoted-text-is-literal",
            "argvdump '$HOME'",
            "argc=1\narg[0]=\"$HOME\"\n",
            0,
        )
        .features(&["native", "word", "single_quote"]),
        Case::native(
            "native/single-quoted-glob-is-literal",
            "argvdump '*.rs'",
            "argc=1\narg[0]=\"*.rs\"\n",
            0,
        )
        .features(&["native", "word", "single_quote", "glob"]),
        Case::native(
            "native/double-quoted-glob-is-literal",
            "argvdump \"*.rs\"",
            "argc=1\narg[0]=\"*.rs\"\n",
            0,
        )
        .features(&["native", "word", "double_quote", "glob"]),
        Case::native(
            "native/double-quoted-escaped-quote",
            r#"argvdump "say \"hi\"""#,
            "argc=1\narg[0]=\"say \\\"hi\\\"\"\n",
            0,
        )
        .features(&["native", "word", "double_quote", "backslash"]),
        Case::native(
            "native/double-quoted-escaped-backslash",
            r#"argvdump "a\\b""#,
            "argc=1\narg[0]=\"a\\\\b\"\n",
            0,
        )
        .features(&["native", "word", "double_quote", "backslash"]),
        Case::native(
            "native/double-quoted-escaped-dollar",
            r#"argvdump "a\$b""#,
            "argc=1\narg[0]=\"a$b\"\n",
            0,
        )
        .features(&["native", "word", "double_quote", "backslash"]),
        Case::native(
            "native/double-quoted-escaped-braces",
            r#"argvdump "a\{b\}c""#,
            "argc=1\narg[0]=\"a{b}c\"\n",
            0,
        )
        .features(&["native", "word", "double_quote", "backslash"]),
        Case::native(
            "native/double-quoted-preserves-backslash-n",
            r#"argvdump "a\nb""#,
            "argc=1\narg[0]=\"a\\\\nb\"\n",
            0,
        )
        .features(&["native", "word", "double_quote", "backslash"]),
        Case::native(
            "native/double-quoted-preserves-backslash-t",
            r#"argvdump "a\tb""#,
            "argc=1\narg[0]=\"a\\\\tb\"\n",
            0,
        )
        .features(&["native", "word", "double_quote", "backslash"]),
        Case::native(
            "native/double-quoted-line-continuation",
            "argvdump \"a\\\nb\"",
            "argc=1\narg[0]=\"ab\"\n",
            0,
        )
        .features(&["native", "word", "double_quote", "backslash"]),
        // An escape is the other way to make a metacharacter literal. Native
        // keeps the backslash in the value — its documented unknown-escape rule
        // (`docs/LANGUAGE.md`, "backslash escapes in words"), so unquoted regexes
        // survive intact. This pins that `\*.rs` is *not* expanded, and records
        // the text divergence from POSIX's `*.rs` as deliberate.
        Case::native(
            "native/escaped-glob-is-not-expanded",
            "argvdump \\*.rs",
            "argc=1\narg[0]=\"\\\\*.rs\"\n",
            0,
        )
        .features(&["native", "word", "glob"]),
        // A mixed word: the quoted `*` is data while the trailing `*.rs` is a
        // pattern. A per-word "was this quoted" flag cannot express this, which
        // is why provenance is per fragment.
        Case::native(
            "native/mixed-quoted-and-active-glob",
            "argvdump foo'*'*.rs",
            "argc=1\narg[0]=\"foo**.rs\"\n",
            0,
        )
        .features(&["native", "word", "glob", "single_quote"]),
        // ...and the mirror image, where the quoted part is the tail.
        Case::native(
            "native/active-glob-with-quoted-tail",
            "argvdump \"foo\"*'.rs'",
            "argc=1\narg[0]=\"foo*.rs\"\n",
            0,
        )
        .features(&["native", "word", "glob", "double_quote", "single_quote"]),
        // Quoting protects against more than globbing: nothing inside a quoted
        // fragment is structural syntax.
        Case::native(
            "native/quoted-list-syntax-is-literal",
            "argvdump \"[a,b]\"",
            "argc=1\narg[0]=\"[a,b]\"\n",
            0,
        )
        .features(&["native", "word", "double_quote"]),
        Case::native(
            "native/quoted-operators-are-literal",
            "argvdump 'a;b|c>d'",
            "argc=1\narg[0]=\"a;b|c>d\"\n",
            0,
        )
        .features(&["native", "word", "single_quote"]),
        // Quoted braces are not brace syntax, and a quoted tilde does not expand.
        Case::native(
            "native/quoted-braces-are-literal",
            "argvdump '{a,b}'",
            "argc=1\narg[0]=\"{a,b}\"\n",
            0,
        )
        .features(&["native", "word", "single_quote", "brace"]),
        Case::native(
            "native/quoted-tilde-is-literal",
            "argvdump '~'",
            "argc=1\narg[0]=\"~\"\n",
            0,
        )
        .features(&["native", "word", "single_quote", "tilde"]),
        // A command's own bytes must reach the terminal unaltered, even when they
        // happen to be JSON. fsh used to decode them and re-encode them in its
        // tagged representation here; that is only correct when a downstream
        // stage consumes a value.
        Case::native(
            "native/json-stdout-is-reencoded",
            "emit --stdout '{\"a\":1}'",
            "{\"a\":1}\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .features(&["native", "output"]),
        // External stdout is preserved byte-for-byte at the terminal boundary.
        // Decoding it into a typed value is only for a downstream consumer, so
        // these must all come back exactly as the command wrote them.
        Case::native(
            "native/json-array-stdout-survives",
            "emit --stdout '[1,2]'",
            "[1,2]\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .features(&["native", "output"]),
        Case::native(
            "native/json-nested-stdout-survives",
            "emit --stdout '{\"a\":{\"b\":2}}'",
            "{\"a\":{\"b\":2}}\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .features(&["native", "output"]),
        Case::native(
            "native/plain-stdout-survives",
            "emit --stdout hello",
            "hello\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .features(&["native", "output"]),
        // Quoting protects more than globbing: a quoted word is scanned for no
        // structural syntax at all, so a `[...]` list or a `;`/`|` inside
        // quotes stays one literal argument.
        Case::native(
            "native/quoted-bracket-list-stays-literal",
            "emit --stdout '{\"a\":[1,2]}'",
            "{\"a\":[1,2]}\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .features(&["native", "word", "output"]),
        // ...but a downstream stage does get a value, which is the whole point of
        // a typed pipeline. `@json` renders a decoded Map as `{\"a\":5}`; had the
        // bytes merely passed through as a string it would be a quoted JSON string.
        Case::native(
            "native/downstream-stage-receives-typed-value",
            "emit --stdout '{\"a\":5}' | @json",
            "{\"a\":5}\n",
            0,
        )
        .engines(&[Engine::Native])
        .features(&["native", "output", "pipeline"]),
    ]
}

/// Default (auto) mode: the path a coding agent's shell commands actually take.
///
/// Expectations here are POSIX ones, because that is the compatibility target.
/// A native-engine defect that silently replaces correct POSIX behaviour shows
/// up as a failure in this group.
fn dispatch_cases() -> Vec<Case> {
    vec![
        // Each of these also asserts *which* engine ran: native owns this
        // syntax, so diverting would be a routing defect even where the bytes
        // happen to agree.
        Case::posix("dispatch/simple-command", "argvdump abc")
            .engines(&[Engine::Auto])
            .expect_engine(Engine::Native),
        Case::posix("dispatch/single-quoted-glob", "argvdump '*.rs'")
            .engines(&[Engine::Auto])
            .expect_engine(Engine::Native),
        Case::posix("dispatch/double-quoted-glob", "argvdump \"*.rs\"")
            .engines(&[Engine::Auto])
            .expect_engine(Engine::Native),
        // The mixed case under the POSIX oracle: only the unquoted `*.rs` may
        // glob, so bash/dash judge whether the quoted `*` really stayed data.
        Case::posix("dispatch/mixed-quoted-glob", "argvdump foo'*'*.rs")
            .engines(&[Engine::Auto])
            .expect_engine(Engine::Native),
        Case::posix("dispatch/quoted-metacharacters", "argvdump 'a;b|c>d'")
            .engines(&[Engine::Auto])
            .expect_engine(Engine::Native),
        // Field splitting is the one divergence with *no* routing expectation,
        // and that is a decision, not an oversight: it depends on the value, not
        // the syntax — `$BAR` splits, `$HOME` does not, and both are the same
        // word. Diverting every unquoted expansion would hijack native's own
        // words, where a list in argv is deliberately one argument. See
        // `crates/fshell-engine/src/compat.rs` and the divergence table in
        // `docs/LANGUAGE.md`. Native does not field-split: an unquoted expansion
        // stays the typed value it already was, so the router keeps this native
        // rather than divert to POSIX. POSIX owns splitting (see
        // `word/unquoted-parameter-splits`).
        Case::native(
            "dispatch/unquoted-parameter-stays-one-value",
            "BAR=\"a b\"; argvdump $BAR",
            "argc=1\narg[0]=\"a b\"\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        Case::posix("dispatch/assignment-looking-argument", "argvdump n=1")
            .engines(&[Engine::Auto]),
        // `$?` is native's own feature, and these assert the value *and* that
        // the router keeps native: a wrong status is a bug to fix, never a
        // reason to divert, or the routing table would become a list of native
        // bugs. Both engines run, so the value and the dispatch are independent.
        Case::posix(
            "dispatch/exit-status-after-failure",
            "false; emit --stdout \"rc=$?\"",
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        Case::posix(
            "dispatch/exit-status-after-success",
            "true; emit --stdout \"rc=$?\"",
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        Case::posix(
            "dispatch/exit-status-cleared-by-success",
            "false; true; emit --stdout \"rc=$?\"",
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        // The status must belong to the *previous* command, not to a later one:
        // the second `emit` reports the first `emit`, so `a=1` then `b=0`.
        Case::posix(
            "dispatch/exit-status-advances-per-statement",
            "false; emit --stdout \"a=$?\"; emit --stdout \"b=$?\"",
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        Case::posix(
            "dispatch/exit-status-embedded-in-word",
            "false; emit --stdout \"x$?y\"",
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        Case::posix(
            "dispatch/exit-status-unquoted",
            "false; emit --stdout rc=$?",
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        Case::posix(
            "dispatch/exit-status-from-instrument",
            "emit --exit 5; emit --stdout \"rc=$?\"",
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        // `$?` must survive the whole expansion phase: another command running
        // inside the same word must not disturb it. This carries a literal
        // expectation rather than an oracle because the reference shells
        // disagree — bash re-reads `$?` after each substitution (`c=0`), while
        // dash keeps the value the word started with (`c=1`) and native matches
        // dash. On a contested expectation fsh is pinned to its own documented
        // behaviour rather than judged either way.
        Case::native(
            "dispatch/exit-status-stable-during-expansion",
            "false; emit --stdout \"a=$? b=$(echo sub) c=$?\"",
            "a=1 b=sub c=1\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        // A command that cannot be found is a command failure, not an engine
        // failure: native reports it and carries on with 127, exactly as the
        // reference shells do. These judge the behaviour an oracle can model,
        // and never the wording — the diagnostic text is native's own.
        Case::posix(
            "dispatch/command-not-found-continues",
            "nosuchcmd; emit --stdout \"rc=$?\"",
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        Case::posix(
            "dispatch/command-not-found-status-then-success",
            "nosuchcmd; emit --stdout \"a=$?\"; emit --stdout \"b=$?\"",
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        // The status must drive control flow rather than being swallowed, and
        // the script's own exit status must carry it.
        Case::posix(
            "dispatch/command-not-found-short-circuits-and",
            "nosuchcmd && emit --stdout no",
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        Case::posix(
            "dispatch/command-not-found-runs-or",
            "nosuchcmd || emit --stdout recovered",
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        // Non-fatal is not the same as ignored: it still trips errexit.
        Case::posix(
            "dispatch/command-not-found-sets-errexit",
            "set -e; nosuchcmd; emit --stdout unreachable",
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        Case::posix(
            "dispatch/command-not-found-in-substitution",
            "emit --stdout \"x=$(nosuchcmd)y\"",
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        // An engine-level failure is not the command's to redirect: taking the
        // diagnostic's text away must not turn an abort into a status. Native's
        // own error report is the shell's, so it is not suppressible — only the
        // *command's* diagnostics follow the command's stderr.
        Case::native(
            "dispatch/hard-error-aborts-despite-redirect",
            "cd /nonexistent 2>/dev/null; emit --stdout unreachable",
            "",
            1,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        Case::native(
            "dispatch/hard-error-aborts-after-earlier-output",
            "emit --stdout f; cd /nonexistent 2>/dev/null",
            "f\n",
            1,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        // A hard failure belongs to the pipeline, not to whichever stage happens
        // to be last: a failure in a non-last stage still fails the pipeline. The
        // sibling records no output, so the case does not depend on whether a
        // cancelled stage got to print before the abort.
        Case::native(
            "dispatch/hard-error-in-pipeline-aborts",
            "cd /nonexistent | emit --exit 0; emit --stdout unreachable",
            "",
            1,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        // A diagnostic is not pipeline data: `cat` receives nothing to print, and
        // the failure does not leak into stdout.
        Case::posix(
            "dispatch/command-not-found-is-not-piped-as-data",
            "nosuchcmd | cat; echo $?",
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        // POSIX takes the last command's status, so a later successful stage keeps
        // the pipeline at 0 even though an earlier stage could not be found.
        Case::posix(
            "dispatch/command-not-found-keeps-last-stage-status",
            "nosuchcmd | emit --exit 0; echo $?",
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        // `pipefail` reports the rightmost stage that failed, not the last stage —
        // the shape a single shared status slot could not represent. It is a bash
        // extension, so bash is the only reference that can judge it.
        Case::posix(
            "dispatch/pipefail-reports-earlier-failure",
            "set -o pipefail; emit --exit 1 | emit --exit 0; echo $?",
        )
        .bash_only()
        .engines(&[Engine::Native, Engine::Auto]),
        Case::posix(
            "dispatch/pipefail-reports-last-failure",
            "set -o pipefail; emit --exit 0 | emit --exit 1; echo $?",
        )
        .bash_only()
        .engines(&[Engine::Native, Engine::Auto]),
        // Without `pipefail`, the pipeline's status is its last stage's.
        Case::posix(
            "dispatch/last-stage-status-without-pipefail",
            "emit --exit 1 | emit --exit 0; echo $?",
        )
        .engines(&[Engine::Native, Engine::Auto]),
        // A composite statement publishes the status of the statement it executed
        // last, rather than resetting to 0 because it returned normally.
        Case::native(
            "dispatch/composite-if-publishes-body-status",
            "if true { emit --exit 4 }; emit --stdout \"rc=$?\"",
            "rc=4\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        Case::native(
            "dispatch/composite-if-without-a-branch-succeeds",
            "if false { emit --exit 4 }; emit --stdout \"rc=$?\"",
            "rc=0\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        Case::native(
            "dispatch/composite-if-else-publishes-the-taken-branch",
            "if false { emit --exit 4 } else { emit --exit 5 }; emit --stdout \"rc=$?\"",
            "rc=5\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        Case::native(
            "dispatch/composite-nested-publishes-last-child",
            "if true { if true { emit --exit 10 } }; emit --stdout \"rc=$?\"",
            "rc=10\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        Case::native(
            "dispatch/composite-loop-publishes-body-status",
            "while true { emit --exit 6; break }; emit --stdout \"rc=$?\"",
            "rc=6\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        Case::native(
            "dispatch/composite-loop-without-iterations-succeeds",
            "while false { emit --exit 6 }; emit --stdout \"rc=$?\"",
            "rc=0\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        Case::native(
            "dispatch/composite-for-publishes-body-status",
            "for x in \"1\" { emit --exit 7 }; emit --stdout \"rc=$?\"",
            "rc=7\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        Case::native(
            "dispatch/composite-unsafe-publishes-body-status",
            "unsafe { emit --exit 9 }; emit --stdout \"rc=$?\"",
            "rc=9\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        Case::native(
            "dispatch/composite-try-publishes-body-status",
            "try { emit --exit 8 } catch |e| { }; emit --stdout \"rc=$?\"",
            "rc=8\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        // A command that exists but cannot be started is POSIX status 126, and the
        // statement carries on.
        Case::posix(
            "dispatch/unexecutable-command-is-status-126",
            "./notexec; echo \"rc=$?\"",
        )
        .files(&["notexec"])
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        // A destination that cannot be written is a command-level failure: status
        // 1 and the statement continues. Bash and dash disagree on the status
        // (1 vs 2), so fsh follows bash and pins that choice here rather than
        // against a contested oracle.
        Case::native(
            "redirect/unwritable-stdout-continues",
            "emit --stdout x > /nonexistent/dir/f; echo \"rc=$?\"",
            "rc=1\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        Case::native(
            "redirect/unwritable-stderr-continues",
            "emit --stdout x 2> /nonexistent/dir/f; echo \"rc=$?\"",
            "rc=1\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        // A command substitution contributes output *bytes*: newline-separated,
        // with its trailing newlines stripped, exactly as POSIX strips `$(...)`.
        Case::posix(
            "dispatch/substitution-text-strips-trailing-newline",
            "emit --stdout \"x=$(emit --stdout hi)y\"",
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        Case::posix(
            "dispatch/substitution-keeps-inner-newlines",
            "emit --stdout \"x=$(printf 'a\\nb')y\"",
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native),
        // Native command substitution parses its body as a statement list, so a
        // redirection, a command list, an and-or list and an assignment are all
        // accepted the way POSIX allows.
        Case::posix(
            "dispatch/substitution-accepts-a-redirection",
            "emit --stdout \"x=$(ls > /dev/null)y\"",
        )
        .engines(&[Engine::Native]),
        Case::posix(
            "dispatch/substitution-accepts-two-commands",
            "emit --stdout \"x=$(emit --stdout a; emit --stdout b)y\"",
        )
        .engines(&[Engine::Native]),
        Case::posix(
            "dispatch/substitution-accepts-and-or",
            "emit --stdout \"x=$(emit --stdout a && emit --stdout b)y\"",
        )
        .engines(&[Engine::Native]),
        Case::posix(
            "dispatch/substitution-accepts-an-assignment",
            "echo \"[$(PATH=/tmp)]\"",
        )
        .engines(&[Engine::Native]),
        Case::posix("dispatch/printf-format-reuse", "printf '%s\\n' a b c")
            .engines(&[Engine::Native, Engine::Auto])
            .expect_engine(Engine::Native),
        // The same rule for a command substitution: its bytes are one value in
        // native, and POSIX owns splitting them.
        Case::native(
            "dispatch/unquoted-command-substitution-stays-one-value",
            "argvdump $(printf 'a b')",
            "argc=1\narg[0]=\"a b\"\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native)
        .features(&["dispatch", "command_substitution"]),
        Case::posix(
            "dispatch/real-world-find",
            "find . -name '*.rs' -not -path './target/*' | /usr/bin/sort",
        )
        .engines(&[Engine::Auto])
        .features(&["dispatch", "real_world", "glob"]),
        Case::posix(
            "dispatch/if-then-fi",
            "if [ -f a.rs ]; then emit --stdout yes; fi",
        )
        .engines(&[Engine::Auto]),
        Case::posix(
            "dispatch/for-do-done",
            "for f in *.rs; do emit --stdout \"$f\"; done",
        )
        .engines(&[Engine::Auto]),
    ]
}

/// Commands of the shape coding agents and build scripts actually emit.
///
/// These are the practical benchmark: the minimised cases above identify the
/// semantic bug, these prove the workflow works end to end.
fn real_world_cases() -> Vec<Case> {
    vec![
        Case::posix_only(
            "real_world/find-excluding-target",
            "find . -name '*.rs' -not -path './target/*' | /usr/bin/sort",
        )
        .features(&["real_world", "find", "glob"]),
        Case::posix_only(
            "real_world/find-argv-shape",
            "argvdump -name '*.rs' -not -path './target/*'",
        )
        .features(&["real_world", "find", "glob"]),
        Case::posix_only(
            "real_world/grep-recursive-include",
            "/usr/bin/grep -rn 'fn a' --include='*.rs' . | /usr/bin/sort",
        )
        .features(&["real_world", "grep", "glob"]),
        Case::posix_only("real_world/printf-format-reuse", "printf '%s\\n' a b c")
            .features(&["real_world", "printf"]),
        Case::posix_only(
            "real_world/command-substitution-into-argument",
            "count=$(printf '%s\\n' a b c | /usr/bin/wc -l); emit --stdout \"count=$count\"",
        )
        .features(&["real_world", "command_substitution"]),
        Case::posix_only(
            "real_world/exit-status-check",
            "/usr/bin/false; emit --stdout \"rc=$?\"",
        )
        .features(&["real_world", "exit_status"]),
        Case::posix_only(
            "real_world/scoreboard-style-one-liner",
            "count=$(/usr/bin/find . -name '*.rs' -not -path './target/*' | /usr/bin/wc -l); emit --stdout \"files=$count\"",
        )
        .features(&["real_world", "find", "command_substitution"]),
        Case::posix_only(
            "real_world/heredoc-into-stdin",
            "while read -r line; do emit --stdout \"<$line>\"; done <<EOF\nfirst\nsecond\nEOF",
        )
        .features(&["real_world", "heredoc", "loop"]),
        Case::posix_only("real_world/herestring", "cat <<< hello")
            // `<<<` is a bash extension: dash rejects it, so bash is the oracle.
            .bash_posix_only()
            .features(&["real_world", "herestring", "bash_extension"]),
        Case::posix_only(
            "real_world/extended-test",
            "if [[ -f a.rs ]]; then emit --stdout yes; fi",
        )
        .bash_posix_only()
        .features(&["real_world", "bash_extension"]),
        Case::posix_only("real_world/process-substitution", "cat <(emit --stdout sub)")
            .bash_only()
            .features(&["real_world", "bash_extension"])
            .known_failure(
                "posix-process-substitution-silently-empty",
                "process substitution is accepted but produces nothing, so \
                 `cat <(cmd)` prints an empty stream instead of the command's \
                 output — a silent mis-execution rather than an explicit rejection",
                &[Engine::Posix],
            ),
        Case::posix_only(
            "real_world/conditional-execution",
            "[ -f Cargo.toml ] && emit --stdout manifest || emit --stdout absent",
        )
        .features(&["real_world", "and_if", "test"]),
    ]
}

#[test]
fn conformance_word_model() {
    assert_suite(&word_model_cases());
}

#[test]
fn conformance_composition() {
    assert_suite(&composition_cases());
}

#[test]
fn conformance_redirection() {
    assert_suite(&redirection_cases());
}

#[test]
fn conformance_native_word_model() {
    assert_suite(&native_word_model_cases());
}

#[test]
fn conformance_dispatch() {
    assert_suite(&dispatch_cases());
}

#[test]
fn conformance_real_world() {
    assert_suite(&real_world_cases());
}

/// The 19 cases migrated from `scripts/posix-compat-baseline/`, which is retired.
///
/// The legacy suite committed `.stdout`/`.status` files because it had no oracle
/// available at run time. Here the expectation is *derived* from `bash --posix`
/// and `dash` instead, so the committed goldens are deliberately not copied: they
/// could only drift from the references.
///
/// One property of the legacy suite is preserved beyond stdout and exit status:
/// it required stderr to be **empty** for all three shells. `Oracle::Posix`
/// makes the reference stderr the expectation, so `compare_stderr()` keeps that
/// guarantee — a leaked fsh diagnostic fails the case.
///
/// These run against the POSIX engine only, as they did before. Whether
/// shell-shaped input also reaches the POSIX engine in default (auto) mode is a
/// separate concern, covered by the `dispatch` group.
fn legacy_migrated_cases() -> Vec<Case> {
    vec![
        Case::posix_only(
            "legacy/01-quoting",
            r##"set -- 'two words' '' '*'
printf '<%s>\n' "$@""##,
        )
        .compare_stderr()
        .features(&["legacy", "word", "positional"]),
        Case::posix_only(
            "legacy/02-parameter-expansion",
            r##"unset baseline_value
printf '%s|%s|%s\n' "${baseline_value:-fallback}" "${baseline_value:=assigned}" "$baseline_value""##,
        )
        .compare_stderr()
        .features(&["legacy", "parameter"]),
        Case::posix_only(
            "legacy/03-control-flow",
            r##"sum=0
for n in 1 2 3 4
do
    sum=$((sum + n))
done
if [ "$sum" -eq 10 ]; then
    printf 'sum=%s\n' "$sum"
else
    exit 1
fi"##,
        )
        .compare_stderr()
        .features(&["legacy", "loop", "if"]),
        Case::posix_only(
            "legacy/04-functions-and-positionals",
            r##"show_pair() {
    printf '<%s>|<%s>\n' "$1" "$2"
}
show_pair 'left side' right"##,
        )
        .compare_stderr()
        .features(&["legacy", "function", "positional"]),
        Case::posix_only(
            "legacy/05-command-substitution",
            r##"value=$(printf 'first\nsecond\n\n')
printf '<%s>\n' "$value""##,
        )
        .compare_stderr()
        .features(&["legacy", "command_substitution"]),
        Case::posix_only(
            "legacy/06-case-pattern",
            r##"value=report.txt
case $value in
    *.txt) printf 'text file\n' ;;
    *) printf 'other\n' ;;
esac"##,
        )
        .compare_stderr()
        .features(&["legacy", "case"]),
        Case::posix_only(
            "legacy/07-pipeline-status",
            r##"false | true
printf '%s\n' "$?""##,
        )
        .compare_stderr()
        .features(&["legacy", "pipeline", "exit_status"]),
        Case::posix_only(
            "legacy/08-heredoc",
            r##"value=world
cat <<EOF
hello $value
EOF"##,
        )
        .compare_stderr()
        .features(&["legacy", "heredoc"]),
        Case::posix_only(
            "legacy/09-redirection",
            r##"printf 'first\n' > baseline-output.txt
printf 'second\n' >> baseline-output.txt
cat baseline-output.txt"##,
        )
        .compare_stderr()
        .features(&["legacy", "redirect", "append"]),
        Case::posix_only(
            "legacy/10-and-or",
            r##"false && printf 'wrong\n'
true || printf 'wrong\n'
printf 'and-or-ok\n'"##,
        )
        .compare_stderr()
        .features(&["legacy", "and_if", "or_if"]),
        Case::posix_only(
            "legacy/11-ifs-field-splitting",
            r##"IFS=:
value=alpha:beta:gamma
set -- $value
printf '<%s>\n' "$@""##,
        )
        .compare_stderr()
        .features(&["legacy", "field_split", "ifs"]),
        Case::posix_only(
            "legacy/12-pathname-expansion",
            // The fixture resets before every invocation, so only the files this
            // script creates are present — stronger isolation than the legacy
            // suite's single shared working directory, which coupled cases.
            r##": > baseline-a.glob
: > baseline-b.glob
: > baseline-c.txt
set -- baseline-*.glob
printf '<%s>\n' "$@""##,
        )
        .compare_stderr()
        .features(&["legacy", "glob", "positional"]),
        Case::posix_only(
            "legacy/13-parameter-pattern-removal",
            r##"path=src/module/file.c
printf '%s|%s\n' "${path##*/}" "${path%/*}""##,
        )
        .compare_stderr()
        .features(&["legacy", "parameter"]),
        Case::posix_only(
            "legacy/14-arithmetic-expansion",
            r##"left=7
right=5
printf '%s\n' "$((left * 3 + right))""##,
        )
        .compare_stderr()
        .features(&["legacy", "arithmetic"]),
        Case::posix_only(
            "legacy/15-subshell-isolation",
            r##"value=outer
(
    value=inner
    printf 'inside=%s\n' "$value"
)
printf 'outside=%s\n' "$value""##,
        )
        .compare_stderr()
        .features(&["legacy", "subshell"]),
        Case::posix_only(
            "legacy/16-quoted-heredoc",
            r##"value=expanded
cat <<'EOF'
$value
EOF"##,
        )
        .compare_stderr()
        .features(&["legacy", "heredoc", "single_quote"]),
        Case::posix_only(
            "legacy/17-break-continue",
            r##"for n in 1 2 3 4
do
    [ "$n" -eq 2 ] && continue
    [ "$n" -eq 4 ] && break
    printf '%s\n' "$n"
done"##,
        )
        .compare_stderr()
        .features(&["legacy", "loop", "break", "continue"]),
        Case::posix_only(
            "legacy/18-negated-command",
            r##"if ! false; then
    printf 'negation-ok\n'
fi"##,
        )
        .compare_stderr()
        .features(&["legacy", "if", "negation"]),
        Case::posix_only(
            "legacy/19-assignment-no-globbing",
            // A glob metacharacter in an assignment value must stay literal:
            // this is the historical regression the legacy suite documented.
            r##": > baseline-a.glob
: > baseline-b.glob
value=baseline-*.glob
printf '<%s>\n' "$value""##,
        )
        .compare_stderr()
        .features(&["legacy", "assignment", "glob"]),
    ]
}

#[test]
fn conformance_legacy_migrated() {
    assert_suite(&legacy_migrated_cases());
}

/// Routing assertions: which engine auto mode commits input to, and why.
///
/// Output equality cannot see dispatch. A case can pass because auto mode
/// reached the *wrong* engine that happened to agree, and a routing regression
/// can hide behind a `known_failure` that was going to fail anyway. These cases
/// read `$FSH_ENGINE_TRACE` instead, so the two dimensions are judged
/// separately — semantic result and routing result.
///
/// The rule the table must obey: native handling input is the default, and
/// diversion needs a capability native deliberately does not implement. `$?` is
/// therefore asserted *native* and left to the phase that fixes it. Routing is
/// not a place to park native bugs.
fn routing_cases() -> Vec<Case> {
    vec![
        // Native owns this. Diverting because POSIX would also accept it is
        // exactly how a router decays into "prefer POSIX whenever possible".
        Case::native(
            "routing/native-syntax-stays-native",
            "let x = 5; echo $x",
            "5\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native)
        .features(&["routing", "native"]),
        // A native parse failure is a routing decision, not a fallback accident:
        // POSIX is reached because native rejected the syntax outright, and the
        // reason recorded says so.
        Case::posix(
            "routing/posix-only-syntax-routes-to-posix",
            "case x in x) echo y;; esac",
        )
        .engines(&[Engine::Auto])
        .expect_engine(Engine::Posix)
        .features(&["routing", "posix"]),
        Case::posix("routing/brace-group-routes-to-posix", "{ echo hi; }")
            .engines(&[Engine::Auto])
            .expect_engine(Engine::Posix)
            .features(&["routing", "posix"]),
        // Native has no POSIX special parameters, so `"$@"` is diverted and the
        // POSIX engine agrees with bash and dash. Both dimensions now pass: this
        // row was the table's first fix.
        Case::posix("routing/special-parameter-prefers-posix", "echo \"$@\"")
            .engines(&[Engine::Posix, Engine::Auto])
            .expect_engine(Engine::Posix)
            .features(&["routing", "posix", "parameter"]),
        // `~user` is not a native tilde form, so it is diverted. Routing is now
        // correct; the *value* is still wrong, but for a different reason — the
        // POSIX frontend's own `~user` defect, pinned just below. Keeping the
        // two apart is the point: the router must not look green while the
        // result is wrong.
        Case::posix("routing/tilde-user-prefers-posix", "echo ~root")
            .engines(&[Engine::Auto])
            .bash_posix_only()
            .expect_engine(Engine::Posix)
            .features(&["routing", "posix", "tilde"]),
        // Guard cases for the tilde row: the table must divert `~user` and
        // nothing else about `~`. Native tilde forms stay native, and a quoted
        // `~root` is literal in POSIX too, so it is not a divergence at all.
        Case::native(
            "routing/bare-tilde-stays-native",
            "argvdump ~",
            "argc=1\narg[0]=\"{HOME}\"\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native)
        .features(&["routing", "native", "tilde"]),
        Case::native(
            "routing/tilde-home-path-stays-native",
            "argvdump ~/src",
            "argc=1\narg[0]=\"{HOME}/src\"\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native)
        .features(&["routing", "native", "tilde"]),
        Case::native(
            "routing/quoted-tilde-user-stays-native",
            "argvdump '~root'",
            "argc=1\narg[0]=\"~root\"\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native)
        .features(&["routing", "native", "tilde"]),
        // The POSIX frontend's own `~user` defect, pinned so it cannot hide
        // behind the routing row above: diverting to POSIX would not fix it.
        Case::posix("routing/tilde-user-posix-gap", "echo ~root")
            .engines(&[Engine::Posix])
            .bash_posix_only()
            .features(&["routing", "posix", "tilde"]),
    ]
}

#[test]
fn conformance_routing() {
    assert_suite(&routing_cases());
}
