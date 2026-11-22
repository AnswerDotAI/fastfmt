use fastfmt::{compact, fastfmt, guarded_config, import_paths, join_comment_lines, merge_imports, shorten_paths};

#[test]
fn merges_adjacent_imports_without_crossing_boundaries() {
    for (src, want) in [
        ("use ::exhash::EditResult;\nuse ::exhash::FileEdit;\nuse ::exhash::FileError;", "use ::exhash::{EditResult, FileEdit, FileError};"),
        ("use x::{A, B,};\nuse x::C;", "use x::{A, B, C};"), ("use x::A as Local;\nuse x::B;", "use x::{A as Local, B};"),
        ("use x::*;\nuse x::A;", "use x::{*, A};"), ("pub(crate) use x::A;\npub(crate) use x::B;", "pub(crate) use x::{A, B};"),
        ("fn f() { use x::A; use x::B; }", "fn f() { use x::{A, B}; }"),
    ] {
        assert_eq!(merge_imports(src), want);
        assert_eq!(merge_imports(want), want);
    }
    for src in [
        "use x::A;\n// keep\nuse x::B;", "use x::A; // keep\nuse x::B;", "use x::A;\nuse x::B; // keep", "#[cfg(test)]\nuse x::A;\nuse x::B;",
        "use x::A;\n#[cfg(test)]\nuse x::B;", "/// docs\nuse x::A;\nuse x::B;", "use x::A;\n\nuse x::B;", "use x::A;\npub use x::B;",
        "#[cfg(test)]\n// explanation\nuse x::A;\nuse x::B;", "/// docs\n// explanation\nuse x::A;\nuse x::B;", "use x::A;\nuse y::B;",
        "use x::{A /* keep */};\nuse x::B;", "use x::{};\nuse x::B;", "macro_rules! m { () => { use x::A; use x::B; }; }",
    ] { assert_eq!(merge_imports(src), src); }
}

#[test]
fn shortens_path_nodes_without_touching_opaque_text() {
    let imports = import_paths("crate::float::Native\n").unwrap();
    for (src, want) in [
        ("type T = crate :: float :: Native;", "type T = Native;"), ("type T = crate::float::Native<u8>;", "type T = Native<u8>;"),
        ("fn f() { crate::float::Native::new(); }", "fn f() { Native::new(); }"),
        ("fn f() { crate::float::Native::<u8>::new(); }", "fn f() { Native::<u8>::new(); }"),
        ("type T = <crate::float::Native as Trait>::Item;", "type T = <Native as Trait>::Item;"),
        ("type T = Wrap<crate::float::Native>;", "type T = Wrap<Native>;"),
    ] {
        let out = shorten_paths(src, &imports).unwrap();
        assert_eq!(out, format!("use crate::float::Native;\n{want}"));
        assert_eq!(shorten_paths(&out, &imports).unwrap(), out);
    }
    let src = r##"// crate::float::Native
const S: &str = "crate::float::Native";
const R: &str = r#"crate::float::Native"#;
type T = crate /* keep */ ::float::Native;
type U = crate::float::NativeExtra;
use crate::float::Native;
#[crate::float::Native]
fn f() { m!(crate::float::Native); }
macro_rules! m { () => { $crate::float::Native }; }
"##;
    assert_eq!(shorten_paths(src, &imports).unwrap(), src);
}

#[test]
fn rejects_conflicting_imports_and_items() {
    let imports = import_paths("crate::float::Native").unwrap();
    for declaration in [
        "use other::Native;", "use other::{Native};", "use other::X as Native;", "use other::{self as Native};", "struct Native;", "type Native = u8;",
        "mod Native {}", "extern crate other as Native;",
    ] {
        let src = format!("{declaration}\nfn f() -> crate::float::Native {{ todo!() }}");
        let err = shorten_paths(&src, &imports).unwrap_err();
        assert!(err.starts_with("1:"), "{err}");
        assert!(err.contains("`Native` conflicts"), "{err}");
    }
    let shared = import_paths("crate::float::Native\nother::Native").unwrap();
    assert!(shorten_paths("type A = crate::float::Native; type B = other::Native;", &shared).unwrap_err().contains("both bind `Native`"));
    let src = "mod a { type T = crate::float::Native; } mod b { type T = other::Native; }";
    let out = shorten_paths(src, &shared).unwrap();
    assert!(out.contains("use crate::float::Native;") && out.contains("use other::Native;"));
    assert_eq!(shorten_paths(&out, &shared).unwrap(), out);
    for invalid in ["Native", "crate::float::*", "crate::float::{Native}", "crate::float::Native as N", "crate::float::Native; fn f() {}"] {
        assert!(import_paths(invalid).is_err(), "{invalid}");
    }
}

/// Format `src` at `width`, asserting that a second pass changes nothing.
fn stable(src: &str, width: usize) -> String {
    let out = fastfmt(src, "2021", width, false).unwrap();
    if out != src { assert_eq!(fastfmt(&out, "2021", width, false).unwrap(), out); }
    out
}

#[test]
fn joins_markdown_prose() {
    let cases = [
        ("// One line\n// continues here.\n", "// One line continues here.\n"),
        ("/// One line\n/// continues here.\nfn f() {}\n", "/// One line continues here.\nfn f() {}\n"),
        ("//! One line\n//! continues here.\n", "//! One line continues here.\n"),
        ("fn f() {\n    // café\n    // 日本語\n}\n", "fn f() {\n    // café 日本語\n}\n"), ("//one\r\n//two", "//one two"),
        ("// - First item\n//   continues.\n// - Second item\n", "// - First item continues.\n// - Second item\n"),
        ("// 3. First item\n//    continues.\n// 4. Second item\n", "// 3. First item continues.\n// 4. Second item\n"),
        ("// > - First item\n// >   continues.\n// > - Second item\n", "// > - First item continues.\n// > - Second item\n"),
        ("// > > Nested quote\n// > > continues.\n", "// > > Nested quote continues.\n"),
        ("// > Lazy quote\n// continues with \\*literal markup.\n", "// > Lazy quote continues with \\*literal markup.\n"),
        ("// Before `some code`\n// and [a link](https://example.com).\n", "// Before `some code` and [a link](https://example.com).\n"),
        ("// First paragraph.\n//\n// Second paragraph\n// continues.\n", "// First paragraph.\n//\n// Second paragraph continues.\n"),
        ("// [^n]: A footnote\n//     continues here.\n", "// [^n]: A footnote continues here.\n"),
    ];
    for (src, want) in cases {
        let out = join_comment_lines(src);
        assert_eq!(out, want, "{src}");
        assert_eq!(join_comment_lines(&out), out);
    }
    let src = "/// A paragraph that already exceeds the width\n/// continues without wrapping.\nfn f() {}\n";
    let out = fastfmt(src, "2021", 40, true).unwrap();
    assert!(out.contains("/// A paragraph that already exceeds the width continues without wrapping."));
    assert_eq!(fastfmt(&out, "2021", 40, true).unwrap(), out);
}

#[test]
fn preserves_comment_structure() {
    for src in [
        "// ```rust\n// let x = 1;\n// let y = 2;\n// ```\n", "//     let x = 1;\n//     let y = 2;\n", "// - input -> parser\n// - parser -> output\n",
        "// # Heading\n//\n// Multiline heading\n// stays unchanged\n// =================\n", "// | A | B |\n// |---|---|\n// | 1 | 2 |\n",
        "// First line  \n// second line\\\n// third line\n", "// `inline code\n// with a newline`\n", "// <pre>\n// first line\n// second line\n// </pre>\n",
        "// First paragraph\n\n// Second paragraph\n", "// Ordinary comment\n/// Doc comment\nfn f() {}\n",
        "// Outer indentation\n    // Different indentation\n", "fn f() {\n    drop(1); // Trailing comment\n    // Standalone comment\n}\n",
        "fn f() { let s = r#\"\n// Not a comment\n// Still not a comment\n\"#; }\n", "/*\n// Inside a block comment\n// Still inside the block comment\n*/\n",
    ] { assert_eq!(join_comment_lines(src), src, "{src}"); }
}

#[test]
fn joins_statement_with_tail() {
    for body in [
        "x = step(x);\nSome(x)", "let y = compute(x);\ny.finish()", "a();\n42", "a();\nif c { b() } else { d() }", "eprintln!(\"x\");\nreturn;",
        "while a { b(); }\nbreak;",
    ] {
        let src = format!("fn f() {{\n{body}\n}}\n");
        assert_eq!(compact(&src, 150), format!("fn f() {{ {} }}\n", body.replace('\n', " ")));
    }
    for exit in ["break", "continue", "return"] {
        let src = format!(
            "fn f() {{\n    loop {{\n        if cancelled {{\n            error = Some(e);\n            {exit};\n        }}\n        work();\n    }}\n}}\n"
        );
        let out = stable(&src, 160);
        assert!(out.contains(&format!("if cancelled {{ error = Some(e); {exit}; }}")), "{out}");
    }
    let src = "fn f() { before(); let steps = |mut x: f64, step: fn(f64) -> f64| std::iter::from_fn(move || { x = step(x); Some(x) }).take(64); }\n";
    let out = stable(src, 160);
    assert!(out.contains("std::iter::from_fn(move || { x = step(x); Some(x) }).take(64)"), "{out}");
}

#[test]
fn formats_value_chains() {
    let src = "fn f() {\n    let value = if a { one() }\n    else if b {\n        work();\n        more();\n        two()\n    } else { three() };\n    done(value);\n}\n";
    assert_eq!(stable(src, 160), src);
    let src = "fn f() { if a { one() } else if b { two() } else { three() } }\n";
    assert_eq!(stable(src, 160), src);
    let out = stable(src, 40);
    assert!(out.contains("\n    else if b") && out.contains("\n    else { three() }"), "{out}");
    for src in [
        "fn f() {\n    let value = if ready { one() } else {\n        work();\n        more();\n        two()\n    };\n    done(value);\n}\n",
        "fn f() {\n    let value = if ready {\n        work();\n        more();\n        two()\n    } else { three() };\n    done(value);\n}\n",
        "fn f() {\n    if ready {\n        work();\n        more();\n        two();\n    } else {\n        work();\n        more();\n        three();\n    }\n    done();\n}\n",
        "fn f() {\n    if ready { one() } else {\n        work();\n        more();\n        two();\n    }\n    done();\n}\n",
    ] {
        assert_eq!(stable(src, 160), src);
        assert_eq!(compact(&src.replace("} else {", "}\n    else {"), 160), src);
    }
}

#[test]
fn compacts_call_scaffolding() {
    for body in [
        "x.try_iter()?.enumerate().map(|(i, x)| {\n        if i % 1024 == 0 { py.check_signals()?; }\n        rational(&x?, &fraction)\n    }).collect()",
        "let xs = xs.into_iter().enumerate().map(|(i, x)| {\n        if i % 1024 == 0 { py.check_signals()?; }\n        fraction.call1((x.numer().clone(), x.denom().clone()))\n    }).collect::<PyResult<Vec<_>>>()?;",
        "output(py, x.iter().zip(&y).enumerate().map(|(i, (x, y))| {\n        if i % 1024 == 0 { py.check_signals()?; }\n        f(x, y)\n    }).collect::<PyResult<_>>()?)",
        "output(a_rather_long_argument_to_the_outer_call, &mut |(i, x)| {\n        if i % 1024 == 0 { py.check_signals()?; }\n        rational(&x?, &fraction)\n    })",
    ] { let src = format!("fn f() {{\n    {body}\n}}\n"); for width in [120, 160] { assert_eq!(stable(&src, width), src); } }
    let src = "fn f() {\n    output(py, xs.map(|x| {\n        let y = a(x);\n        b(y);\n        y\n    }).collect::<Result<Vec<_>, E>>()?)\n}\n";
    for width in [40, 55, 80] {
        let out = stable(src, width);
        assert!(out.lines().all(|line| line.len() <= width), "{out}");
        assert!(out.contains("let y = a(x);\n"));
        if width >= 55 { assert!(out.contains("}).collect::<Result<Vec<_>, E>>()?)"), "{out}"); }
    }
    for body in ["// keep this comment\n        work(x)", "let s = r#\"first\nsecond\"#;\n        work(s)"] {
        let src = format!("fn f() {{\n    output(xs.map(|x| {{\n        {body}\n    }}).collect())\n}}\n");
        assert_eq!(compact(&src, 80), src);
        stable(&src, 80);
    }
    let src = "fn f() {\n    output(\n        xs.map(|x| {\n            let s = r#\"first\nsecond\"#;\n            work(s)\n        }).collect(),\n    )\n}\n";
    assert_eq!(compact(src, 160), src); // moving the callback left must not reindent its literal
    let src = "fn f() {\n    let pair = (name, json!({\n        \"data\": event.get(\"data\").cloned().unwrap_or_else(|| json!({})),\n        \"metadata\": event.get(\"metadata\").cloned().unwrap_or_else(|| json!({})),\n    }));\n    consume(pair);\n}\n";
    assert_eq!(stable(src, 100), src);
}

#[test]
fn joins_nested_imports() {
    let want = "use rustymath::{Arg, Unary, distribution::{self as dist, standard::*}};\n";
    for width in [want.len() - 1, want.len() - 2] {
        let out = fastfmt(want, "2024", width, false).unwrap();
        assert_eq!(out == want, width == want.len() - 1, "{out}");
        assert_eq!(fastfmt(&out, "2024", width, false).unwrap(), out);
    }
    let src = "fn f() {\n    g(\n        (1,),\n        (1, 2,),\n        \"comma, }\",\n    )\n}\n";
    assert!(compact(src, 100).contains("g((1,), (1, 2), \"comma, }\")"));
}

#[test]
fn sizes_bodies() {
    for line in [
        "fn gcd<'py>(py: Python<'py>, x: &Bound<'py, PyAny>, y: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyList>> { binary(py, x, y, |x, y| Ok(exact::gcd(x, y))) }",
        "fn pdf<'py>(&self, py: Python<'py>, x: PyArrayLikeDyn<'py, f64, AllowTypeChange>) -> Bound<'py, PyAny> { each(py, x, dist::Pdf(self.0)) }",
        "fn cdf<'py>(&self, py: Python<'py>, x: PyArrayLikeDyn<'py, f64, AllowTypeChange>) -> Bound<'py, PyAny> { each(py, x, dist::Cdf(self.0)) }",
    ] {
        let src = format!("{line}\n");
        assert_eq!(stable(&src, 160), src);
    }
    for (len, width, join) in [(120, 160, true), (121, 160, false), (120, 119, false)] {
        let want = format!("fn f() {{ {}() }}\n", "a".repeat(len - 6));
        let out = stable(&want, width);
        assert_eq!(out == want, join, "{out}");
    }
    let src = format!("fn f() {{\n    if c {{\n        {}();\n    }}\n    done();\n}}\n", "x".repeat(115));
    assert_eq!(compact(&src, 160), src); // an if body has the same 120-character budget
}

#[test]
fn joins_simple_macro_arms() {
    let src = "macro_rules! floats {\n    ($a:expr, $t:ident => $body:expr) => {\n        match $a.dtype().char() {\n            b'e' => {\n                type $t = f16;\n                $body\n            }\n            b'f' => {\n                type $t = f32;\n                $body\n            }\n            _ => {\n                type $t = f64;\n                $body\n            }\n        }\n    };\n}\n";
    let out = stable(src, 160);
    for (arm, ty) in [("b'e'", "f16"), ("b'f'", "f32"), ("_", "f64")] { assert!(out.contains(&format!("{arm} => {{ type $t = {ty}; $body }}")), "{out}"); }
    for body in ["let a = 1;\nlet b = 2;\n$body", "if ready { work() }\n$body", "$($x;)*", "// Keep this comment\n$body", "r#\"first\nsecond\"#"] {
        let src = format!("macro_rules! m {{ () => {{ match x {{ _ => {{\n{body}\n}} }} }}; }}\n");
        assert_eq!(compact(&src, 160), src);
    }
}

#[test]
fn packs_continuations_and_lists() {
    let src = r#"struct Cell<'a> { layout: &'a MatrixLoop, pointers: &'a [usize], outer: &'a [npy_intp], core: &'a [npy_intp], dims: &'a [usize], i: usize }
enum Kind { E, R, C, I }
enum Tall<'a, T> { Q, #[cfg(native)] R, Project(&'a [T], usize) }
trait Cells {
type Shared;
type Per;
type Cell<'a> where Self: 'a;
unsafe fn cell<'a>(shared: &'a Self::Shared, per: Self::Per, i: usize) -> Self::Cell<'a> where Self: 'a;
}
fn entry<E: Item>(f: Ufunc, kinds: &[Kind]) -> Loop { (Some(f), kinds.iter().map(|k| (match k { Kind::E => E::CODE, Kind::R => E::Real::CODE, Kind::C => E::Complex::CODE, Kind::I => i64::CODE }) as c_char).collect()) }
impl<E: Item> Gufunc<E> for Permanent where E::W: NumAssignRef {
type S = ();
fn each() { work(); more(); done(); }
}
unsafe fn folded<K: Kernel<E, N>, E: Code, const N: usize>(k: &K, n: usize, x: &[*mut c_char], xs: &[npy_intp], o: *mut c_char, os: npy_intp, reducing: bool) -> bool {
work(); more(); true
}
unsafe extern "C" fn inner<M: Make, E: Code, const N: usize>(args: *mut *mut c_char, dims: *mut npy_intp, steps: *mut npy_intp, _: *mut c_void)
where M::K: Kernel<E, N>, {
let n = unsafe { *dims } as usize;
if n == 0 { return; }
work(n)
}
fn f() {
let transposed = Self { op: match self.op { Op::Normal => Op::Transpose, Op::Transpose => Op::Normal, Op::Adjoint => Op::Conjugate, Op::Conjugate => Op::Adjoint }, ..self };
register(m, "minmax", "The minimum and maximum along an axis, preserving the input dtype. Empty lanes give the dtype's extrema identities.", [1, 2], NONE, Some("(n)->(),()"), vec![minmax_entry::<bool>(), minmax_entry::<i8>(), minmax_entry::<u8>(), minmax_entry::<i16>(), minmax_entry::<u16>(), minmax_entry::<i32>(), minmax_entry::<u32>(), minmax_entry::<i64>(), minmax_entry::<u64>(), minmax_entry::<f16>(), minmax_entry::<f32>(), minmax_entry::<f64>()], None)?;
let complex = vec![entry::<Complex<f32>>(looped::<Product<PlusTimes>, Complex<f32>>, &complex), entry::<Complex<f64>>(looped::<Product<PlusTimes>, Complex<f64>>, &complex)];
register(m, "matmul", "The matrix product of each pair of matrices, or for Booleans, whether some k has both a[i,k] and b[k,j]. Integers wrap on overflow.", [2, 1], PRODUCT, [vec![or_and, integer_product::<PlusTimes>()], float_products::<PlusTimes>(), complex].concat())?;
register(m, "maximum_add", "The max-plus product of each pair of real matrices: the greatest a[i,k] + b[k,j].", [2, 1], PRODUCT, [vec![integer_product::<MaxPlus>()], float_products::<MaxPlus>()].concat())?;
register(m, "minimum_add", "The min-plus product of each pair of real matrices: the least a[i,k] + b[k,j].", [2, 1], PRODUCT, [vec![integer_product::<MinPlus>()], float_products::<MinPlus>()].concat())?;
}
"#;
    let out = stable(src, 160);
    assert!(out.lines().all(|line| line.len() <= 160), "{out}");
    assert!(out.contains(src.lines().next().unwrap()), "{out}");
    assert!(out.contains("enum Kind { E, R, C, I }"), "{out}");
    assert!(out.contains("enum Tall<'a, T> { Q, #[cfg(native)] R, Project(&'a [T], usize) }"), "{out}");
    assert!(out.contains("    unsafe fn cell<'a>(shared: &'a Self::Shared, per: Self::Per, i: usize) -> Self::Cell<'a> where Self: 'a;"), "{out}");
    assert!(out.contains("match k { Kind::E => E::CODE, Kind::R => E::Real::CODE, Kind::C => E::Complex::CODE, Kind::I => i64::CODE }"), "{out}");
    assert!(out.contains("}).collect())"), "{out}");
    assert!(out.contains("impl<E: Item> Gufunc<E> for Permanent where E::W: NumAssignRef {"), "{out}");
    assert!(out.contains("(\n    k: &K, n: usize, x: &[*mut c_char], xs: &[npy_intp]"), "{out}");
    assert!(out.contains("reducing: bool) -> bool {"), "{out}");
    assert!(out.contains("}, ..self }"), "{out}");
    assert!(out.contains("\n    let n = unsafe { *dims } as usize;\n"), "{out}");
    assert!(out.contains("minmax_entry::<bool>(), minmax_entry::<i8>()"), "{out}");
    assert!(out.contains("], None)?;"), "{out}");
    assert!(out.contains("[2, 1], PRODUCT,"), "{out}");
    // A parenthesised macro group packs inside its outer lines.
    let src = "fn f() -> bool {\n    matches!(\n        kind,\n        \"a\"\n            | \"b\"\n    )\n}\n";
    assert_eq!(compact(src, 160), "fn f() -> bool {\n    matches!(\n        kind, \"a\" | \"b\"\n    )\n}\n");
}

#[test]
fn guards_rustfmt_config() {
    assert_eq!(guarded_config(""), "disable_all_formatting = true\n");
    assert_eq!(guarded_config("max_width = 120\n"), "max_width = 120\ndisable_all_formatting = true\n");
    assert_eq!(guarded_config("disable_all_formatting = false # no\n"), "disable_all_formatting = true # no\n");
    assert_eq!(guarded_config("disable_all_formatting = true\n"), "disable_all_formatting = true\n");
}

#[test]
fn joins_house_shapes() {
    let cases = [
        ("fn f(x: u8) -> u8 {\n    if x > 1 {\n        return 1;\n    }\n    0\n}\n", "fn f(x: u8) -> u8 { if x > 1 { return 1; } 0 }\n"),
        ("fn f(ready: bool) {\n    if ready {\n        notify();\n    }\n}\n", "fn f(ready: bool) { if ready { notify(); } }\n"), (
            "fn f(fds: Fds) {\n    for fd in fds {\n        let _ = self.poller.delete(borrowed(fd));\n    }\n}\n",
            "fn f(fds: Fds) { for fd in fds { let _ = self.poller.delete(borrowed(fd)); } }\n"),
        ("fn time(&self) -> f64 {\n    self.core.time()\n}\n", "fn time(&self) -> f64 { self.core.time() }\n"), (
            "fn f(c: bool, e: &mut E, h: H) {\n    if c {\n        e.writer = Some(h)\n    } else {\n        e.reader = Some(h)\n    }\n}\n",
            "fn f(c: bool, e: &mut E, h: H) { if c { e.writer = Some(h) } else { e.reader = Some(h) } }\n"), (
            "fn f(c: bool) {\n    if c {\n        a()\n    } else {\n        b()\n    }\n    done();\n}\n",
            "fn f(c: bool) {\n    if c { a() }\n    else { b() }\n    done();\n}\n"),
        ("struct FdEntry<H> {\n    reader: Option<H>,\n    writer: Option<H>,\n}\n", "struct FdEntry<H> { reader: Option<H>, writer: Option<H> }\n"),
        ("enum Rt {\n    Owned(Runtime),\n    Borrowed(Handle),\n}\n", "enum Rt { Owned(Runtime), Borrowed(Handle) }\n"), (
            "fn drain(&self, q: &mut Q) {\n    while let Some(h) = q.pop_front() {\n        ready.push_back(h)\n    }\n}\n",
            "fn drain(&self, q: &mut Q) { while let Some(h) = q.pop_front() { ready.push_back(h) } }\n"), (
            "impl<H> Default for FdEntry<H> {\n    fn default() -> Self {\n        Self { reader: None, writer: None }\n    }\n}\n",
            "impl<H> Default for FdEntry<H> { fn default() -> Self { Self { reader: None, writer: None } } }\n"), (
            "fn h(&self) -> Handle {\n    match self {\n        Rt::Owned(r) => r.handle().clone(),\n        Rt::Borrowed(h) => h.clone(),\n    }\n}\n",
            "fn h(&self) -> Handle { match self { Rt::Owned(r) => r.handle().clone(), Rt::Borrowed(h) => h.clone() } }\n"),
    ];
    for (src, want) in cases { assert_eq!(compact(src, 150), want, "src: {src}") }
}

#[test]
fn keeps_what_must_stay() {
    for src in [
        "fn f() {\n    a();\n    b();\n    c()\n}\n",                                          // two statements plus a tail
        "fn f() {\n    a();\n    fn g() {}\n}\n",                                              // an item is not a tail expression
        "fn f() {\n    a();\n    b();\n}\n",                                                   // two statements in a fn body
        "fn f(ready: bool) {\n    if ready {\n        notify();\n        record();\n        return;\n    }\n}\n", // two actions plus an exit stay multiline
        "fn f(r: R) -> u8 {\n    match r {\n        Err(e) => {\n            log(e);\n            record(e);\n            return 1;\n        }\n        Ok(v) => v,\n    }\n}\n", // multi-statement arm: neither the match nor an enclosing block joins
        "fn f(a: bool) -> u8 {\n    if a { b(); }\n    else { c(); }\n    x\n}\n",   // a statement-position else keeps its own line
        "fn f() {\n    m![a, |x| {\n        // keep\n        work(x);\n        more(x)\n    }];\n}\n", // a closure body that crosses a macro list's last line keeps its comment and statements
        "fn f() {\n    // why\n    a()\n}\n",   // comment must survive
        "fn f() {\n    let s = \"a\nb\";\n}\n", // multiline string literal
    ] { assert_eq!(compact(src, 150), src) }
    let long = format!("fn f() {{\n    {}()\n}}\n", "x".repeat(160));
    assert_eq!(compact(&long, 150), long); // width cap
}
