use std::io::Write;

use super::Error;
use cmake_codegen::simple_flags_to_rust_writer;
use cmake_config::SimpleFlag;
use config::Arch;

const ARMV7_ASM: &str = include_str!("asm/arm.s");
const AARCH64_ASM: &str = include_str!("asm/aarch64.s");
const X86_ASM: &str = include_str!("asm/x86.s");
const X86_64_ASM: &str = include_str!("asm/x86_64.s");

pub struct Generator<'a, 'b, 'c, W: Write + 'a> {
    writer: &'a mut W,
    package_module_name: &'b str,
    arch: &'b Arch,
    flags: &'c [SimpleFlag],
}

impl<'a, 'b, 'c, W: Write> Generator<'a, 'b, 'c, W> {
    pub fn new(
        writer: &'a mut W,
        package_module_name: &'b str,
        arch: &'b Arch,
        flags: &'c [SimpleFlag],
    ) -> Self
    where
        W: Write,
    {
        Self {
            writer,
            package_module_name,
            arch,
            flags,
        }
    }

    pub fn generate(&mut self) -> Result<(), Error> {
        self.generate_features_and_crates()?;
        writeln!(self.writer, "extern crate {};", self.package_module_name)?;

        self.writer.write_all(
            b"
use core::intrinsics;
use core::panic::PanicInfo;
use core::mem;
use sel4_sys::*;\n\n",
        )?;

        self.writer.write_all(b"#[cfg(feature = \"alloc\")]\n")?;
        self.writer.write_all(b"#[global_allocator]\n")?;
        self.writer
            .write_all(b"static ALLOCATOR: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;\n")?;

        self.writer.write_all(
            b"
// include the seL4 kernel configurations
#[allow(dead_code)]
#[allow(non_upper_case_globals)]
pub mod sel4_config {\n",
        )?;
        simple_flags_to_rust_writer(self.flags, self.writer, 4)?;
        self.writer.write_all(b"}\n\n")?;

        self.writer
            .write_all(BOOT_INFO_AND_LANG_ITEM_CODE.as_bytes())?;

        self.writer.write_all(
            b"
fn get_untyped(info: &seL4_BootInfo, size_bytes: usize) -> Option<seL4_CPtr> {
    let mut idx = 0;
    for i in info.untyped.start..info.untyped.end {
        if (1 << info.untypedList[idx].sizeBits) >= size_bytes {
            return Some(i);
        }
        idx += 1;
    }
    None
}

const CHILD_STACK_SIZE: usize = 4096;
static mut CHILD_STACK: *const [u64; CHILD_STACK_SIZE] =
    &[0; CHILD_STACK_SIZE];

        ",
        )?;
        self.generate_main()?;
        let asm = match *self.arch {
            Arch::X86 => X86_ASM,
            Arch::X86_64 => X86_64_ASM,
            Arch::Armv7 => ARMV7_ASM,
            Arch::Aarch64 => AARCH64_ASM,
        };
        writeln!(self.writer, "\nglobal_asm!(r###\"{}\"###);\n", asm)?;
        Ok(())
    }

    fn generate_features_and_crates(&mut self) -> Result<(), Error> {
        writeln!(
            self.writer,
            "// NOTE: this file is generated by fel4
// NOTE: Don't edit it here; your changes will be lost at the next build!
#![no_std]
#![cfg_attr(feature = \"alloc\", feature(alloc))]
#![feature(lang_items, core_intrinsics)]
#![feature(global_asm)]
#![cfg_attr(feature = \"alloc\", feature(global_allocator))]
#![feature(panic_info_message)]\n\n"
        )?;

        self.writer.write_all(b"extern crate sel4_sys;\n")?;
        self.writer.write_all(b"#[cfg(feature = \"alloc\")]\n")?;
        self.writer.write_all(b"extern crate wee_alloc;\n")?;
        self.writer.write_all(b"#[cfg(feature = \"alloc\")]\n")?;
        self.writer.write_all(b"extern crate alloc;\n")?;
        self.writer
            .write_all(b"#[cfg(all(feature = \"test\", feature = \"alloc\"))]\n")?;
        self.writer.write_all(b"#[macro_use]\n")?;
        self.writer.write_all(b"extern crate proptest;\n")?;
        Ok(())
    }

    fn generate_main(&mut self) -> Result<(), Error> {
        self.writer.write_all(
            b"
fn main() {
    let bootinfo = unsafe { &*BOOTINFO };
    let cspace_cap = seL4_CapInitThreadCNode;
    let pd_cap = seL4_CapInitThreadVSpace;
    let tcb_cap = bootinfo.empty.start;
    let untyped = get_untyped(bootinfo, 1 << seL4_TCBBits).unwrap();
    let retype_err: seL4_Error = unsafe {
        seL4_Untyped_Retype(
            untyped,
            api_object_seL4_TCBObject.into(),
            seL4_TCBBits.into(),
            cspace_cap.into(),
            cspace_cap.into(),
            seL4_WordBits.into(),
            tcb_cap,
            1,
        )
    };

    assert!(retype_err == 0, \"Failed to retype untyped memory\");

    let tcb_err: seL4_Error = unsafe {
        seL4_TCB_Configure(
            tcb_cap,
            seL4_CapNull.into(),
            cspace_cap.into(),
            seL4_NilData.into(),
            pd_cap.into(),
            seL4_NilData.into(),
            0,
            0,
        )
    };

    assert!(tcb_err == 0, \"Failed to configure TCB\");

    let stack_base = unsafe { CHILD_STACK as usize };
    let stack_top = stack_base + CHILD_STACK_SIZE;
    let mut regs: seL4_UserContext = unsafe { mem::zeroed() };\n",
        )?;

        match *self.arch {
            Arch::X86 | Arch::X86_64 => {
                writeln!(
                    self.writer,
                    "    #[cfg(feature = \"test\")]
    {{ regs.rip = {}::fel4_test::run as seL4_Word; }}
    #[cfg(not(feature = \"test\"))]
    {{ regs.rip = {}::run as seL4_Word; }}",
                    self.package_module_name, self.package_module_name,
                )?;
                writeln!(self.writer, "    regs.rsp = stack_top as seL4_Word;")?;
            }
            Arch::Armv7 => {
                writeln!(
                    self.writer,
                    "    #[cfg(feature = \"test\")]
    {{ regs.pc = {}::fel4_test::run as seL4_Word; }}
    #[cfg(not(feature = \"test\"))]
    {{ regs.pc = {}::run as seL4_Word; }}",
                    self.package_module_name, self.package_module_name
                )?;
                writeln!(self.writer, "    regs.sp = stack_top as seL4_Word;")?;
            }
            Arch::Aarch64 => {
                writeln!(
                    self.writer,
                    "    #[cfg(feature = \"test\")]
    {{ regs.pc = {}::fel4_test::run as seL4_Word; }}
    #[cfg(not(feature = \"test\"))]
    {{ regs.pc = {}::run as seL4_Word; }}",
                    self.package_module_name, self.package_module_name
                )?;
                writeln!(self.writer, "    regs.sp = stack_top as seL4_Word;")?;
            }
        }
        self.writer.write_all(
            b"
    let _: u32 =
        unsafe { seL4_TCB_WriteRegisters(tcb_cap, 0, 0, 2, &mut regs) };
    let _: u32 = unsafe {
        seL4_TCB_SetPriority(tcb_cap, seL4_CapInitThreadTCB.into(), 255)
    };
    let _: u32 = unsafe { seL4_TCB_Resume(tcb_cap) };
    loop {
        unsafe {
            seL4_Yield();
        }
    }
}
        ",
        )?;
        Ok(())
    }
}

const BOOT_INFO_AND_LANG_ITEM_CODE: &str = r##"
pub static mut BOOTINFO: *mut seL4_BootInfo = (0 as *mut seL4_BootInfo);
static mut RUN_ONCE: bool = false;

#[no_mangle]
pub unsafe extern "C" fn __sel4_start_init_boot_info(
    bootinfo: *mut seL4_BootInfo,
) {
    if !RUN_ONCE {
        BOOTINFO = bootinfo;
        RUN_ONCE = true;
        seL4_SetUserData((*bootinfo).ipcBuffer as usize as seL4_Word);
    }
}

#[lang = "termination"]
trait Termination {
    fn report(self) -> i32;
}

impl Termination for () {
    fn report(self) -> i32 {
        0
    }
}

#[lang = "start"]
#[no_mangle]
fn lang_start<T: Termination + 'static>(
    main: fn() -> T,
    _argc: isize,
    _argv: *const *const u8,
) -> isize {
    main();
    panic!("Root task should never return from main!");
}

#[panic_handler]
#[no_mangle]
fn panic(info: &PanicInfo) -> ! {
    #[cfg(feature = "KernelPrinting")]
    {
        use core::fmt::Write;

        if let Some(loc) = info.location() {
            let _ = write!(
                sel4_sys::DebugOutHandle,
                "panic at {}:{}: ",
                loc.file(),
                loc.line()
            );
        } else {
            let _ = write!(
                sel4_sys::DebugOutHandle,
                "panic: "
            );
        }

        if let Some(fmt) = info.message() {
            let _ = sel4_sys::DebugOutHandle.write_fmt(*fmt);
        }
        let _ = sel4_sys::DebugOutHandle.write_char('\n');

        let _ = write!(
            sel4_sys::DebugOutHandle,
            "----- aborting from panic -----\n"
        );
    }
    unsafe { intrinsics::abort() }
}

#[lang = "eh_personality"]
#[no_mangle]
pub fn eh_personality() {
    #[cfg(feature = "KernelPrinting")]
    {
        use core::fmt::Write;
        let _ = write!(
            sel4_sys::DebugOutHandle,
            "----- aborting from eh_personality -----\n"
        );
    }
    unsafe {
        core::intrinsics::abort();
    }
}

#[lang = "oom"]
#[no_mangle]
pub extern "C" fn oom() -> ! {
    #[cfg(feature = "KernelPrinting")]
    {
        use core::fmt::Write;
        let _ = write!(
            sel4_sys::DebugOutHandle,
            "----- aborting from out-of-memory -----\n"
        );
    }
    unsafe {
        core::intrinsics::abort()
    }
}
"##;
