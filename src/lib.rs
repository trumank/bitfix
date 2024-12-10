use core::slice;
use std::ops::Index;
use std::path::PathBuf;
use std::{ffi::OsStr, fs, marker::PhantomData, path::Path};

use anyhow::{anyhow, Context, Result};
use rlua::{Function, Lua, MultiValue, Table, UserData, Value};
use simple_log::{error, info, LogConfigBuilder};

#[cfg(target_os = "windows")]
proxy_dll::proxy_dll!([d3d9, d3d11, x3daudio1_7, winmm], init);

#[cfg(target_os = "linux")]
mod linux_entry {
    use libc::{c_char, c_int, c_void};

    type MainFunc = extern "C" fn(c_int, *const *const c_char, *const *const c_char) -> c_int;

    type LibcStartMainFunc = extern "C" fn(
        MainFunc,
        c_int,
        *const *const c_char,
        extern "C" fn() -> c_int,
        extern "C" fn(),
        extern "C" fn(),
        *mut c_void,
    ) -> c_int;

    #[no_mangle]
    pub extern "C" fn __libc_start_main(
        main: MainFunc,
        argc: c_int,
        argv: *const *const c_char,
        init: extern "C" fn() -> c_int,
        fini: extern "C" fn(),
        rtld_fini: extern "C" fn(),
        stack_end: *mut c_void,
    ) -> c_int {
        super::init();

        let original_libc_start_main: LibcStartMainFunc = unsafe {
            std::mem::transmute(libc::dlsym(
                libc::RTLD_NEXT,
                c"__libc_start_main".as_ptr() as *const _,
            ))
        };

        original_libc_start_main(main, argc, argv, init, fini, rtld_fini, stack_end)
    }
}

fn init() {
    if let Ok(bin_dir) = setup() {
        info!(
            "bitfix v{}-{} loaded",
            env!("CARGO_PKG_VERSION"),
            &env!("GIT_HASH")[..7]
        );

        unsafe {
            if let Err(e) = patch(bin_dir) {
                error!("{e:#}");
            }
        }
    }
}

fn setup() -> Result<PathBuf> {
    let exe_path = std::env::current_exe()?;
    let bin_dir = exe_path.parent().context("could not find exe parent dir")?;
    let config = LogConfigBuilder::builder()
        .path(bin_dir.join("bitfix.txt").to_str().unwrap()) // TODO why does this not take a path??
        .size(100)
        .roll_count(10)
        .time_format("%Y-%m-%d %H:%M:%S.%f")
        .level("debug")
        .output_file()
        .build();
    simple_log::new(config).map_err(|e| anyhow!("{e}"))?;
    Ok(bin_dir.to_path_buf())
}

unsafe fn patch(bin_dir: PathBuf) -> Result<()> {
    let img = patternsleuth::process::internal::read_image()
        .context("failed to read executable image")?;

    let mut mem = RawMemory::default();
    for section in img.memory.sections() {
        mem.map_page(section.address(), unsafe {
            // HACK PS image offers no way to get memory mutably
            slice::from_raw_parts_mut(section.address() as *mut u8, section.len())
        });
    }

    info!("loading lua patches");

    let patches = load_lua_patches(bin_dir.join("bitfix"))?;

    info!("executing patches");
    exec_patches(&mut mem, patches)?;
    info!("done executing");

    Ok(())
}

trait Memory<'memory>: Index<usize, Output = u8> {
    fn pages(&self) -> usize;
    fn page(&self, index: usize) -> &Page;
    fn page_mut<'s>(&'s mut self, index: usize) -> &'s mut Page<'memory>;
    fn write(&mut self, address: usize, data: u8);
}

struct MatchContext<'wrapper, 'memory, M: Memory<'memory>> {
    address: usize,
    index: usize,
    memory: &'wrapper mut M,
    _phantom: PhantomData<&'memory M>,
}

impl<'memory, M: Memory<'memory>> UserData for MatchContext<'_, 'memory, M> {
    fn add_methods<'lua, T: rlua::UserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("address", |_, this: &Self, ()| Ok(this.address));
        methods.add_method("index", |_, this: &Self, ()| Ok(this.index));
        methods.add_meta_method(rlua::MetaMethod::Index, |_, this: &Self, index: usize| {
            Ok(this.memory[index])
        });
        methods.add_meta_method_mut(
            rlua::MetaMethod::NewIndex,
            |_, this: &mut Self, (index, value): (usize, u8)| {
                this.memory.write(index, value);
                Ok(())
            },
        );
    }
}

#[derive(Debug)]
struct Page<'memory> {
    address: usize,
    memory: &'memory mut [u8],
}

#[derive(Debug, Default)]
struct RawMemory<'memory> {
    pages: Vec<Page<'memory>>,
}
impl<'memory> RawMemory<'memory> {
    fn map_page(&mut self, address: usize, memory: &'memory mut [u8]) {
        self.pages.push(Page { address, memory });
    }
}
impl<'memory> Memory<'memory> for RawMemory<'memory> {
    fn pages(&self) -> usize {
        self.pages.len()
    }
    fn page(&self, index: usize) -> &Page {
        &self.pages[index]
    }
    fn page_mut<'s>(&'s mut self, index: usize) -> &'s mut Page<'memory> {
        &mut self.pages[index]
    }
    fn write(&mut self, index: usize, data: u8) {
        info!("writing {data:02X?} to {index:X?}");
        for Page { address, memory } in &mut self.pages {
            if index >= *address && index < *address + memory.len() {
                let offset = index - *address;
                let write_mem = &mut memory[offset..offset + 1];

                #[cfg(target_os = "windows")]
                {
                    use std::ffi::c_void;
                    use windows::Win32::System::Memory::{
                        VirtualProtect, PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS,
                    };
                    let mut old: PAGE_PROTECTION_FLAGS = Default::default();
                    unsafe {
                        VirtualProtect(
                            write_mem.as_ptr() as *const c_void,
                            write_mem.len(),
                            PAGE_EXECUTE_READWRITE,
                            &mut old,
                        );
                    }

                    write_mem[0] = data;

                    unsafe {
                        VirtualProtect(
                            write_mem.as_ptr() as *const c_void,
                            write_mem.len(),
                            old,
                            &mut old,
                        );
                    }
                }
                #[cfg(target_os = "linux")]
                {
                    unsafe {
                        use libc::{c_void, PROT_EXEC, PROT_READ, PROT_WRITE};

                        let page_size = libc::sysconf(libc::_SC_PAGESIZE) as usize;
                        let start = write_mem.as_ptr() as usize;
                        let start_aligned = (start) & !(page_size - 1);
                        let end_aligned =
                            ((start + write_mem.len()) + page_size - 1) & !(page_size - 1);

                        libc::mprotect(
                            start_aligned as *mut c_void,
                            end_aligned - start_aligned,
                            PROT_READ | PROT_WRITE | PROT_EXEC,
                        );
                    }
                    write_mem[0] = data;
                    // TODO reset protection flags (probably requires parsing /proc maps)
                }
                return;
            }
        }
        panic!("out of bounds")
    }
}
impl Index<usize> for RawMemory<'_> {
    type Output = u8;
    fn index(&self, index: usize) -> &Self::Output {
        for Page { address, memory } in &self.pages {
            if index >= *address && index < *address + memory.len() {
                return &memory[index - address];
            }
        }
        panic!("out of bounds")
    }
}

struct LuaPatch {
    name: String,
    body: String,
}

fn load_lua_patches<P: AsRef<Path>>(path: P) -> Result<Vec<LuaPatch>> {
    let entries = fs::read_dir(&path);
    let mut patches = vec![];
    if let Ok(entries) = entries {
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension() == Some(OsStr::new("lua")) && path.is_file() {
                let patch = LuaPatch {
                    name: path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    body: fs::read_to_string(path)?,
                };
                info!("loaded {:?}", patch.name);
                patches.push(patch);
            }
        }
    } else {
        info!(
            "unable to load lua patches from dir: {}",
            path.as_ref().display()
        );
    }
    Ok(patches)
}

fn exec_patches<'wrapper, 'memory>(
    memory: &'wrapper mut (impl Memory<'memory> + 'memory),
    patches: Vec<LuaPatch>,
) -> Result<()> {
    struct Config<'file, 'lua> {
        file: &'file LuaPatch,
        label: String,
        pattern: String,
        function: Function<'lua>,
    }
    Lua::new().context(|lua| -> Result<()> {
        info!("entered lua context");

        let print = lua.create_function(|lua, args: MultiValue| {
            let tostring = lua.globals().get::<_, Function>("tostring")?;
            let mut buf = String::new();
            let mut iter = args.into_iter().peekable();
            while let Some(arg) = iter.next() {
                let str = tostring.call::<_, String>(arg)?;
                buf.push_str(&str);
                if iter.peek().is_some() {
                    buf.push('\t');
                }
            }
            info!("lua: {buf}");
            Ok(())
        })?;
        lua.globals().set("print", print)?;

        let mut configs = vec![];
        for s in &patches {
            let table = lua
                .load(&s.body)
                .eval::<Table>()
                .with_context(|| format!("in {:?}", s.name))?;
            for pair in table.pairs::<Value, Table>() {
                let (label, v) = pair?;
                configs.push(Config {
                    file: s,
                    label: lua
                        .coerce_string(label)?
                        .with_context(|| "could not coerce {label:?} to string")?
                        .to_str()?
                        .to_string(),
                    pattern: v.get::<_, String>("pattern")?,
                    function: v.get::<_, Function>("match")?,
                });
            }
        }

        let patterns = configs
            .iter()
            .map(|config| patternsleuth_scanner::Pattern::new(&config.pattern))
            .collect::<Result<Vec<_>>>()?;
        let pattern_refs = patterns.iter().collect::<Vec<_>>();

        for i in 0..memory.pages() {
            info!("scanning page: {i}");
            let map = &memory.page(i);
            let results =
                patternsleuth_scanner::scan_pattern(&pattern_refs, map.address, map.memory);
            info!("scan results: {results:X?}");

            for (config, addresses) in configs.iter().zip(results) {
                for (index, address) in addresses.iter().enumerate() {
                    lua.scope(|lua| {
                        let ctx = lua.create_nonstatic_userdata(MatchContext {
                            index,
                            address: *address,
                            memory,
                            _phantom: PhantomData,
                        })?;
                        info!(
                            "calling patcher {}/{}: on {address:X?}",
                            config.file.name, config.label,
                        );
                        config.function.call::<_, ()>(ctx)
                    })?;
                }
            }
        }

        Ok(())
    })
}

#[cfg(test)]
mod test {
    use super::*;

    #[derive(Debug, Default)]
    struct VirtualMemory<'memory> {
        pages: Vec<Page<'memory>>,
    }
    impl<'memory> VirtualMemory<'memory> {
        fn map_page(&mut self, address: usize, memory: &'memory mut [u8]) {
            self.pages.push(Page { address, memory });
        }
    }
    impl<'memory> Memory<'memory> for VirtualMemory<'memory> {
        fn pages(&self) -> usize {
            self.pages.len()
        }
        fn page(&self, index: usize) -> &Page {
            &self.pages[index]
        }
        fn page_mut<'s>(&'s mut self, index: usize) -> &'s mut Page<'memory> {
            &mut self.pages[index]
        }
        fn write(&mut self, index: usize, data: u8) {
            for Page { address, memory } in &mut self.pages {
                if index >= *address && index < *address + memory.len() {
                    memory[index - *address] = data;
                    return;
                }
            }
            panic!("out of bounds")
        }
    }
    impl<'memory> Index<usize> for VirtualMemory<'memory> {
        type Output = u8;
        fn index(&self, index: usize) -> &Self::Output {
            for Page { address, memory } in &self.pages {
                if index >= *address && index < *address + memory.len() {
                    return &memory[index - address];
                }
            }
            panic!("out of bounds")
        }
    }

    #[test]
    fn test_lua() -> Result<()> {
        let config = LogConfigBuilder::builder()
            .level("debug")
            .output_console()
            .build();
        simple_log::new(config).ok();

        let base = 100;
        let mut data = [
            0x00, 0x00, 0x00, 0x00, 0x10, 0x20, 0x99, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let mut memory = VirtualMemory::default();
        memory.map_page(base, &mut data);

        let patches = vec![LuaPatch {
            name: "test".to_string(),
            body: r#"
            {
                patch1 = {
                    pattern = '10 20 ?? 30',
                    match = function(ctx)
                        print(string.format('match found! %s', ctx:address()))
                        print(string.format('first byte: %s', ctx[ctx:address()]))
                        ctx[ctx:address()] = 0x25
                        print('patched')
                    end
                },
                patch2 = {
                    pattern = '00',
                    match = function(ctx)
                        print(string.format('match index; %s', ctx:index()))
                    end
                }
            }
            "#
            .to_string(),
        }];

        exec_patches(&mut memory, patches)?;

        assert_eq!(
            memory.page(0).memory,
            [0x00, 0x00, 0x00, 0x00, 0x25, 0x20, 0x99, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00,]
        );

        Ok(())
    }
}
