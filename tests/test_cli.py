"End-to-end checks of the installed binary: rustfmt runs, then compaction joins."
import json, subprocess, pytest

def package(path, src, edition='edition = "2021"', extra='', name='member'):
    "Write a package at `path` whose `src/lib.rs` holds `src`, and return that file."
    (path/'src').mkdir(parents=True, exist_ok=True)
    (path/'Cargo.toml').write_text(f'[package]\nname = "{name}"\nversion = "0.1.0"\n{edition}\n{extra}\n')
    lib = path/'src/lib.rs'
    lib.write_text(src)
    return lib

def workspace(path, members, extra=''):
    "Write a virtual workspace manifest at `path` listing `members`."
    (path/'Cargo.toml').write_text(f'[workspace]\nmembers = {json.dumps(members)}\nresolver = "2"\n{extra}\n')

def fastfmt(cwd, *args): return subprocess.run(['cargo-fastfmt', *args], cwd=cwd, capture_output=True, text=True)

def formats_clean(cwd, *args):
    "Format in `cwd`, then confirm `--check` finds nothing left to change."
    r = fastfmt(cwd, *args)
    assert r.returncode == 0, r.stderr
    assert fastfmt(cwd, *args, '--check').returncode == 0


# Joined, this is 120 characters: wider than rustfmt's default of 100, and within fastfmt's default of 160.
SOURCE = 'fn wide() {\n    let inner = Arc::new(DapInner { pending: pending.clone(), close: close.clone(), closed: closed.clone() });\n}\n'

def test_fastfmt_binary(tmp_path):
    f = package(tmp_path, SOURCE)
    guard = tmp_path/'rustfmt.toml'
    r = fastfmt(tmp_path, '--check')
    assert r.returncode == 1 and not guard.exists()
    assert 'rustfmt.toml' in r.stdout and 'lib.rs' in r.stdout
    formats_clean(tmp_path)
    assert guard.read_text() == 'disable_all_formatting = true\n'
    out = f.read_text()
    assert out == 'fn wide() { let inner = Arc::new(DapInner { pending: pending.clone(), close: close.clone(), closed: closed.clone() }); }\n'
    subprocess.run(['cargo', 'fmt'], cwd=tmp_path, check=True)
    assert f.read_text() == out
    f.write_text(SOURCE)
    r = fastfmt(tmp_path, '--check')
    assert r.returncode == 1 and 'lib.rs' in r.stdout and 'rustfmt.toml' not in r.stdout


def test_plain_fastfmt_merges_existing_imports(tmp_path):
    source = 'pub use ::std::collections::HashMap;\npub use ::std::collections::HashSet;\npub use ::std::collections::BTreeMap;\n'
    f = package(tmp_path, source)
    assert fastfmt(tmp_path, '--check').returncode == 1 and f.read_text() == source
    formats_clean(tmp_path)
    assert f.read_text() == 'pub use ::std::collections::{BTreeMap, HashMap, HashSet};\n'


def test_ignored_targets(tmp_path):
    subprocess.run(['git', 'init', '-q', str(tmp_path)], check=True)
    source = 'fn f() {\n    work();\n}\n'
    lib = package(tmp_path, source)
    (tmp_path/'.gitignore').write_text('links/\nignored.rs\nrustfmt.toml\nsrc/*.rs\n')
    (tmp_path/'src/.gitignore').write_text('!lib.rs\n')
    (tmp_path/'links/src').mkdir(parents=True)
    subprocess.run(['git', 'init', '-q', str(tmp_path/'links/src')], check=True)
    (tmp_path/'links/Cargo.toml').write_text('invalid Cargo manifest\n')
    untouched = ['links/src/lib.rs', 'ignored.rs', 'src/generated.rs', '.hidden/lib.rs', 'target/lib.rs']
    for name in untouched:
        (tmp_path/name).parent.mkdir(parents=True, exist_ok=True)
        (tmp_path/name).write_text(source)
    def snapshot(): return {f: f.read_bytes() for f in tmp_path.rglob('*') if f.is_file()}
    targets = [('.', False), ('src', False), ('src/lib.rs', True), ('links', False), ('links/src', True), ('links/src/lib.rs', False),
        ('ignored.rs', True), ('src/generated.rs', False)]
    for target, absolute in targets:
        lib.write_text(source)
        arg = str(tmp_path/target) if absolute else target
        formats = target in ('.', 'src', 'src/lib.rs')
        before = snapshot()
        assert fastfmt(tmp_path, arg, '--check').returncode == int(formats), target
        assert snapshot() == before, target
        formats_clean(tmp_path, arg)
        assert lib.read_text() == ('fn f() { work(); }\n' if formats else source), target
        assert all((tmp_path/name).read_text() == source for name in untouched), target
        assert not (tmp_path/'rustfmt.toml').exists() and not (tmp_path/'links/rustfmt.toml').exists(), target


def test_workspace_editions(tmp_path):
    let_chain = 'fn f(x: Option<u8>) { if let Some(x) = x && x > 0 { drop(x); } }\n'
    members = {
        'dotted': ('edition.workspace = true', let_chain),
        'inline': ('edition = { workspace = true }', let_chain),
        'pinned': ("edition = '2021' # member override", 'fn gen() {}\n'),
        'unset': ('', 'fn async() {}\n'),  # Cargo defaults to 2015, not the workspace edition
    }
    workspace(tmp_path, list(members), '[workspace.package]\nedition = "2024"')
    for name, (edition, source) in members.items(): package(tmp_path/name, source, edition, name=name)
    # The only run through Cargo's subcommand dispatch
    subprocess.run(['cargo', 'fastfmt'], cwd=tmp_path, check=True)
    subprocess.run(['cargo', 'fastfmt', '--check'], cwd=tmp_path, check=True)


def test_virtual_workspace(tmp_path):
    subprocess.run(['git', 'init', '-q', str(tmp_path)], check=True)
    workspace(tmp_path, ['member'])
    (tmp_path/'rustfmt.toml').write_text('max_width = 120\n')
    package(tmp_path/'member', 'fn member() {}\n')
    script = tmp_path/'script.rs'
    script.write_text('async fn main() {\n    work().await;\n}\n')
    formats_clean(tmp_path)
    assert script.read_text() == 'async fn main() { work().await; }\n'
    # rustfmt reads the root config for the member as well.
    assert (tmp_path/'rustfmt.toml').read_text() == 'max_width = 120\ndisable_all_formatting = true\n'
    assert not (tmp_path/'member/rustfmt.toml').exists()


@pytest.mark.parametrize('ws_setting,pkg_setting,enabled', [
    ('', '', False),
    ('', 'join_comments = true', True),
    ('join_comments = true', '', True),
    ('join_comments = true', 'join_comments = false', False),
    ('', 'join_comments = "yes"', None),
])
def test_comment_settings(tmp_path, ws_setting, pkg_setting, enabled):
    workspace(tmp_path, ['member'], '[workspace.metadata.fastfmt]\n' + ws_setting)
    source = ('/// A wrapped paragraph\n/// continues here.\n///\n/// - First item\n/// - Second item\n'
        '///\n/// ```rust\n/// let x = 1;\n/// let y = 2;\n/// ```\n///\n/// Heading\n/// =======\n'
        '///\n/// First line  \n/// second line\\\n/// third line\nfn f() {}\n')
    f = package(tmp_path/'member', source, extra='[package.metadata.fastfmt]\n' + pkg_setting)
    if enabled is None:
        r = fastfmt(tmp_path)
        assert r.returncode == 2 and 'join_comments must be a boolean' in r.stderr
        assert f.read_text() == source
        return
    formats_clean(tmp_path)
    assert f.read_text() == (source.replace('paragraph\n/// continues', 'paragraph continues') if enabled else source)
    f.write_text(source)
    assert fastfmt(tmp_path, '--check').returncode == int(enabled)


def test_shorten_modules(tmp_path):
    source = '''//! Module docs stay before imports.
#![allow(dead_code, unused_imports)]
use ::std::collections::BTreeMap;
/// This belongs to `float`, not the new imports.
pub mod float { pub struct Native; }
pub mod other { pub struct Native; }
pub mod recipe { pub trait NativeOps {} }
pub type Bag = ::std::collections::HashMap<(), ::std::collections::HashSet<()>>;
impl crate::recipe::NativeOps for crate::float::Native {}
pub mod nested {
    //! Inner docs also stay before imports.
    #![allow(unused_imports)]
    use crate::{float::{self, Native}, recipe::NativeOps as OtherOps};
    /// Function docs stay attached.
    pub fn make() -> crate::float::Native { crate::float::Native }
    pub fn bound<T: crate::recipe::NativeOps>(x: std::sync::Arc<T>) -> std::sync::Arc<T> { x }
    macro_rules! make_native { () => { $crate::float::Native }; }
    pub fn from_macro() -> crate::float::Native { make_native!() }
    pub const TOKENS: &str = stringify!(crate::float::Native);
    pub const STRING: &str = "crate::float::Native";
}
mod separate;
'''
    f = package(tmp_path, source)
    separate = tmp_path/'src/separate.rs'
    separate.write_text('pub fn make() -> crate::other::Native { crate::other::Native }\n')
    mapping = tmp_path/'imports.txt'
    mapping.write_text('::std::collections::HashSet\ncrate::float::Native\ncrate::other::Native\n::std::collections::HashMap\ncrate::recipe::NativeOps\nstd::sync::Arc\n')
    args = ('shorten', str(mapping))
    r = fastfmt(tmp_path, *args, '--check')
    assert r.returncode == 1 and f.read_text() == source and not (tmp_path/'rustfmt.toml').exists()
    assert 'lib.rs' in r.stdout and 'separate.rs' in r.stdout
    subprocess.run(['cargo', 'fastfmt', *args], cwd=tmp_path, check=True)
    assert fastfmt(tmp_path, *args, '--check').returncode == 0
    out = f.read_text()
    assert out.count('use crate::float::Native;') == 1  # The nested grouped import is reused.
    assert out.count('use crate::recipe::NativeOps;') == 2  # An alias does not bind NativeOps.
    assert out.count('use std::sync::Arc;') == 1  # Only the nested module uses Arc.
    assert 'use ::std::collections::{BTreeMap, HashMap, HashSet};' in out  # Normal formatting merges existing and new imports.
    assert 'pub type Bag = HashMap<(), HashSet<()>>;' in out
    assert 'pub fn make() -> Native { Native }' in out and 'x: Arc<T>) -> Arc<T>' in out
    assert '$crate::float::Native' in out and 'stringify!(crate::float::Native)' in out
    assert '"crate::float::Native"' in out
    assert out.index('#![allow(dead_code, unused_imports)]') < out.index('use crate::float::Native;')
    assert '/// This belongs to `float`, not the new imports.\npub mod float' in out
    assert '/// Function docs stay attached.\n    pub fn make' in out
    assert separate.read_text() == 'use crate::other::Native;\npub fn make() -> Native { Native }\n'
    subprocess.run(['cargo', 'check', '--offline', '--quiet'], cwd=tmp_path, check=True, timeout=60)


def test_shorten_collision_writes_nothing(tmp_path):
    f = package(tmp_path, 'fn f() -> crate::float::Native { todo!() }\n')
    later = tmp_path/'src/z.rs'
    later.write_text('use other::Native;\nfn f() -> crate::float::Native { todo!() }\n')
    mapping = tmp_path/'imports.txt'
    mapping.write_text('crate::float::Native\n')
    before = {p: p.read_bytes() for p in (f, later, mapping)}
    r = fastfmt(tmp_path, 'shorten', str(mapping))
    assert r.returncode == 2 and 'z.rs:1:' in r.stderr and '`Native` conflicts' in r.stderr
    assert {p: p.read_bytes() for p in before} == before and not (tmp_path/'rustfmt.toml').exists()


def test_candidates_report(tmp_path):
    source = '''use crate::{data::Thing, old::Clash};
struct Declared;
fn f() -> crate::data::Thing { crate::data::Thing::new() }
type Nested = crate::data::Thing<crate::params::Param>;
type Other = crate::params::Param;
fn conflict() -> other::Clash { other::Clash::new() }
fn declared() -> other::Declared { other::Declared::new() }
fn shared() -> second::Thing { second::Thing::new() }
fn variants() { match x { variants::Kind::First => variants::Kind::Second } }
type Associated = Local::Item;
type AssociatedAgain = Local::Item;
const C: u8 = source::SOME_CONST;
const D: u8 = source::SOME_CONST;
mod child {
    use crate::data::Thing;
    fn f() -> crate::data::Thing { crate::data::Thing::new() }
}
// noise::Comment noise::Comment
const S: &str = "noise::String noise::String";
const R: &str = r#"noise::Raw noise::Raw"#;
#[noise::Attribute]
#[noise::Attribute]
fn lowercase() { helper::function(); helper::function(); }
fn functions() {
    std::thread::sleep(std::time::Duration::from_millis(1));
    std::thread::spawn(|| {});
    let _ = std::mem::size_of::<std::time::Duration>();
    let _ = std::mem::align_of::<u8>();
    let callback = std::thread::sleep;
}
macro_rules! m { () => { noise::Macro::f(noise::Macro) }; }
fn opaque() { m!(noise::Args, noise::Args); }
pub(in visibility::Ignored) fn a() {}
pub(in visibility::Ignored) fn b() {}
type Single = unique::OnlyOnce;
type Distributed = distributed::Once;
'''
    (tmp_path/'lib.rs').write_text(source)
    (tmp_path/'more.rs').write_text('fn extra() -> crate::data::Thing { crate::data::Thing::new() } type D = distributed::Once;')
    (tmp_path/'.ignore').write_text('ignored.rs\n')
    (tmp_path/'ignored.rs').write_text('type T = ignored::Type; type U = ignored::Type;')
    (tmp_path/'Cargo.toml').write_text('not valid TOML')  # Discovery needs neither Cargo metadata nor rustfmt.
    (tmp_path/'rustfmt.toml').write_text('disable_all_formatting = false\n')
    before = {p: p.read_bytes() for p in tmp_path.iterdir()}
    r = fastfmt(tmp_path, 'candidates', '--json')
    assert r.returncode == 0, r.stderr
    rows = {row['path']: row for row in json.loads(r.stdout)}
    expected = {'crate::data::Thing', 'crate::params::Param', 'other::Clash', 'other::Declared', 'second::Thing', 'variants::Kind',
        'std::thread', 'std::mem', 'std::time::Duration'}
    assert set(rows) == expected
    assert list(rows) == ['crate::data::Thing', *sorted(expected - {'crate::data::Thing'})]
    thing = rows['crate::data::Thing']
    assert (thing['uses'], thing['modules'], thing['max_file']) == (7, 3, 5)
    assert [(f['file'], f['uses'], f['modules']) for f in thing['files']] == [('./lib.rs', 5, 2), ('./more.rs', 2, 1)]
    assert all((row['uses'], row['modules'], row['max_file']) == (2, 1, 2) for path, row in rows.items() if path != 'crate::data::Thing')
    assert all(row['files'][0]['example'].startswith('./lib.rs:') for row in rows.values())
    assert all(rows[path]['files'][0]['conflict'].startswith('./lib.rs:') for path in ('other::Clash', 'other::Declared', 'second::Thing'))
    assert thing['shared_name'] and all(f['conflict'] is None for f in thing['files']) and rows['second::Thing']['shared_name']
    human = fastfmt(tmp_path, 'candidates')
    assert human.returncode == 0 and 'max/file' in human.stdout and '(5)' in human.stdout and '[conflict:' in human.stdout
    selected = fastfmt(tmp_path, 'candidates', '--json', 'more.rs')
    assert selected.returncode == 0 and [(r['path'], r['uses'], r['max_file']) for r in json.loads(selected.stdout)] == [('crate::data::Thing', 2, 2)]
    assert {p: p.read_bytes() for p in tmp_path.iterdir()} == before
