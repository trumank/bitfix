use std::ops::Index;
use std::path::PathBuf;
use std::{
    ffi::{c_void, OsStr},
    fs,
    marker::PhantomData,
    path::Path,
};

use anyhow::{anyhow, Context, Result};
use rlua::{Function, Lua, Table, UserData, Value};
use simple_log::{error, info, LogConfigBuilder};
use windows::{
    Win32::Foundation::*,
    Win32::System::{
        LibraryLoader::GetModuleHandleA,
        Memory::{VirtualProtect, PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS},
        ProcessStatus::{GetModuleInformation, MODULEINFO},
        SystemServices::*,
        Threading::{GetCurrentProcess, GetCurrentThread, QueueUserAPC},
    },
};

// x3daudio1_7.dll
#[no_mangle]
#[allow(non_snake_case, unused_variables)]
extern "system" fn X3DAudioCalculate() {}
#[no_mangle]
#[allow(non_snake_case, unused_variables)]
extern "system" fn X3DAudioInitialize() {}

// d3d9.dll
#[no_mangle]
#[allow(non_snake_case, unused_variables)]
extern "system" fn D3DPERF_EndEvent() {}
#[no_mangle]
#[allow(non_snake_case, unused_variables)]
extern "system" fn D3DPERF_BeginEvent() {}

#[no_mangle]
#[allow(non_snake_case, unused_variables)]
extern "system" fn DllMain(dll_module: HMODULE, call_reason: u32, _: *mut ()) -> bool {
    unsafe {
        match call_reason {
            DLL_PROCESS_ATTACH => {
                QueueUserAPC(Some(init), GetCurrentThread(), 0);
            }
            DLL_PROCESS_DETACH => (),
            _ => (),
        }

        true
    }
}

unsafe extern "system" fn init(_: usize) {
    info!("patcher loaded");

    if let Ok(bin_dir) = setup() {
        if let Err(e) = patch(bin_dir) {
            error!("{e:#}");
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
    let module = GetModuleHandleA(None).context("could not find main module")?;
    let process = GetCurrentProcess();

    let mut mod_info = MODULEINFO::default();
    GetModuleInformation(
        process,
        module,
        &mut mod_info as *mut _,
        std::mem::size_of::<MODULEINFO>() as u32,
    );

    let mut memory = RawMemory::default();
    memory.map_page(
        mod_info.lpBaseOfDll as usize,
        std::slice::from_raw_parts_mut(
            mod_info.lpBaseOfDll as *mut u8,
            mod_info.SizeOfImage as usize,
        ),
    );

    info!("loading lua patches");

    let patches = load_lua_patches(bin_dir.join("bitfix"))?;

    info!("executing patches");
    exec_patches(&mut memory, patches)?;
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
    memory: &'wrapper mut M,
    _phantom: PhantomData<&'memory M>,
}

impl<'wrapper, 'memory, M: Memory<'memory>> UserData for MatchContext<'wrapper, 'memory, M> {
    fn add_methods<'lua, T: rlua::UserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("address", |_, this: &Self, ()| Ok(this.address));
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
                return;
            }
        }
        panic!("out of bounds")
    }
}
impl<'memory> Index<usize> for RawMemory<'memory> {
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
            .enumerate()
            .map(|(index, config)| {
                Ok((index, patternsleuth_scanner::Pattern::new(&config.pattern)?))
            })
            .collect::<Result<Vec<_>>>()?;
        let pattern_refs = patterns
            .iter()
            .map(|(name, pattern)| (name, pattern))
            .collect::<Vec<_>>();

        for i in 0..memory.pages() {
            info!("scanning page: {i}");
            let map = &memory.page(i);
            let results =
                patternsleuth_scanner::scan_memchr_lookup(&pattern_refs, map.address, map.memory);
            info!("scan results: {results:X?}");

            for (index, address) in results {
                lua.scope(|lua| {
                    let ctx = lua.create_nonstatic_userdata(MatchContext {
                        address,
                        memory,
                        _phantom: PhantomData,
                    })?;
                    let config = &configs[*index];
                    info!(
                        "calling patcher {}/{}: on {address:X?}",
                        config.file.name, config.label,
                    );
                    configs[*index].function.call::<_, ()>(ctx)
                })?;
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
                patch = {
                    pattern = '10 20 ?? 30',
                    match = function(ctx)
                        print(string.format('match found! %s', ctx:address()))
                        print(string.format('first byte: %s', ctx[ctx:address()]))
                        ctx[ctx:address()] = 0x25
                        print('patched')
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
