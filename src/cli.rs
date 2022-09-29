//! # LSD - The kiwi Compiler
//! 
//! This program implements the CLI frontend to the kiwi programming
//! language. LSD can compile kiwi's various source languages to
//! the supported targets provided by the compiler.
use kiwi::{
    lir::*,
    parse::*,
    targets::{self, Target},
    vm::*,
    LOGO_WITH_COLOR, TAGLINE, *,
};
use clap::*;
use std::{
    fmt,
    fs::{read_to_string, write},
};

/// The target options to compile the given source code to.
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum TargetType {
    /// Execute the source code in the interpreter.
    Run,
    /// Compile to the core variant of the assembly language.
    CoreASM,
    /// Compile to the standard variant of the assembly language.
    StdASM,
    /// Compile to the core variant of the virtual machine.
    CoreVM,
    /// Compile to the standard variant of the virtual machine.
    StdVM,
    /// Compile to C source code (GCC only).
    C,
    /// Compile to x86 assembly.
    X86,
}

/// The source language options to compile.
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum SourceType {
    /// Compile LIR code.
    LowIR,
    /// Compile core variant assembly code.
    CoreASM,
    /// Compile standard variant assembly code.
    StdASM,
    /// Compile core variant virtual machine code.
    CoreVM,
    /// Compile standard variant virtual machine code.
    StdVM,
}

/// The argument parser for the CLI.
#[derive(Parser, Debug)]
#[clap(author, version, before_help = TAGLINE, about = Some(LOGO_WITH_COLOR), long_about = Some(LOGO_WITH_COLOR), max_term_width=70)]
struct Args {
    /// The input file to compiler.
    #[clap(value_parser)]
    input: String,

    /// The file to write the output of the compiler to.
    #[clap(short, long, value_parser, default_value = "out")]
    output: String,

    /// The source language to compile.
    #[clap(short, value_parser, default_value = "low-ir")]
    source_type: SourceType,

    /// The target language to compile to.
    #[clap(short, value_parser, default_value = "run")]
    target_type: TargetType,

    /// The number of cells allocated for the call stack.
    #[clap(short, long, value_parser, default_value = "8192")]
    call_stack_size: usize,
}

/// The types of errors returned by the CLI.
enum Error {
    /// Error in reading source or writing generated code.
    IO(std::io::Error),
    /// Error parsing the source code.
    Parse(String),
    /// Error generated when compiling LIR code.
    LirError(lir::Error),
    /// Error generated when assembling input code.
    AsmError(asm::Error),
    /// Error generated by the interpreter executing input code.
    InterpreterError(String),
    /// Error when building the virtual machine code for a given target.
    BuildError(String),
    /// Invalid source code (expected core but got standard).
    InvalidSource(String),
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::IO(e) => write!(f, "IO error: {:?}", e),
            Error::Parse(e) => write!(f, "Parse error: {}", e),
            Error::AsmError(e) => write!(f, "Assembly error: {:?}", e),
            Error::LirError(e) => write!(f, "LIR error: {:?}", e),
            Error::InterpreterError(e) => write!(f, "Interpreter error: {}", e),
            Error::BuildError(e) => write!(f, "Build error: {}", e),
            Error::InvalidSource(e) => write!(f, "Invalid source: {}", e),
        }
    }
}

/// Compile a given source language to virtual machine code.
fn compile_source_to_vm(
    src: String,
    src_type: SourceType,
    call_stack_size: usize,
) -> Result<Result<kiwi::vm::CoreProgram, kiwi::vm::StandardProgram>, Error> {
    match src_type {
        SourceType::StdVM => {
            // Simply parse the virtual machine code
            parse_vm(src).map_err(Error::Parse)
        }
        SourceType::CoreVM => {
            // Parse the virtual machine code
            match parse_vm(src).map_err(Error::Parse)? {
                // If we got a core program back, return it.
                Ok(prog) => Ok(Ok(prog)),
                // Otherwise, our core program was actually a standard program. Throw an error.
                Err(_) => Err(Error::InvalidSource(
                    "expected core VM program, got standard VM program".to_string(),
                )),
            }
        }
        SourceType::StdASM => {
            // Parse the assembly code.
            // Then, assembly the program with the given recursion depth,
            // and return the virtual machine output.
            match parse_asm(src).map_err(Error::Parse)? {
                Ok(prog) => Ok(Ok(prog
                    .assemble(call_stack_size)
                    .map_err(Error::AsmError)?)),
                Err(prog) => Ok(Err(prog
                    .assemble(call_stack_size)
                    .map_err(Error::AsmError)?)),
            }
        }
        SourceType::CoreASM => {
            // Parse the assembly code.
            match parse_asm(src).map_err(Error::Parse)? {
                // If we got back a core program, assembly it and return the virtual machine code.
                Ok(prog) => Ok(Ok(prog
                    .assemble(call_stack_size)
                    .map_err(Error::AsmError)?)),
                // Otherwise, our core program was actually a standard program. Throw an error.
                Err(_) => Err(Error::InvalidSource(
                    "expected core assembly program, got standard assembly program".to_string(),
                )),
            }
        }
        SourceType::LowIR => {
            // Parse the lower intermediate representation code.
            match parse_lir(src)
                .map_err(Error::Parse)?
                .compile()
                .map_err(Error::LirError)?
            {
                // If we got back a valid program, assemble it and return the result.
                Ok(asm_code) => Ok(Ok(asm_code
                    .assemble(call_stack_size)
                    .map_err(Error::AsmError)?)),
                Err(asm_code) => Ok(Err(asm_code
                    .assemble(call_stack_size)
                    .map_err(Error::AsmError)?)),
            }
        }
    }
}

/// Compile code in a given source language to assembly code.
fn compile_source_to_asm(
    src: String,
    src_type: SourceType,
) -> Result<Result<kiwi::asm::CoreProgram, kiwi::asm::StandardProgram>, Error> {
    match src_type {
        // If the source language is standard assembly, then parse it and return it.
        SourceType::StdASM => parse_asm(src).map_err(Error::Parse),
        // If the source language is core assembly, then parse it and return it if it's actually a core variant program.
        // Otherwise, throw an error.
        SourceType::CoreASM => match parse_asm(src).map_err(Error::Parse)? {
            Ok(prog) => Ok(Ok(prog)),
            Err(_) => Err(Error::InvalidSource(
                "expected core assembly program, got standard assembly program".to_string(),
            )),
        },
        // If the source language is LIR, parse it and compile it to assembly code.
        SourceType::LowIR => parse_lir(src)
            .map_err(Error::Parse)?
            .compile()
            .map_err(Error::LirError),
        // If the source language is a virtual machine program,
        // then we cannot compile it to assembly. Throw an error.
        SourceType::CoreVM | SourceType::StdVM => Err(Error::InvalidSource(
            "cannot compile a core VM program to assembly".to_string(),
        )),
    }
}

/// Compile code in a given source language to a given target language.
fn compile(
    src: String,
    src_type: SourceType,
    target: TargetType,
    output: String,
    call_stack_size: usize,
) -> Result<(), Error> {
    match target {
        // If the target is `Run`, then compile the code and execute it with the interpreter.
        TargetType::Run => match compile_source_to_vm(src, src_type, call_stack_size)? {
            // If the code is core variant virtual machine code
            Ok(vm_code) => {
                CoreInterpreter::new(StandardDevice)
                    .run(&vm_code)
                    .map_err(Error::InterpreterError)?;
            }
            // If the code is standard variant virtual machine code
            Err(vm_code) => {
                StandardInterpreter::new(StandardDevice)
                    .run(&vm_code)
                    .map_err(Error::InterpreterError)?;
            }
        },
        // If the target is C source code, then compile the code to virtual machine code,
        // and then use the C target implementation to build the output source code.
        TargetType::C => write_file(
            format!("{output}.c"),
            match compile_source_to_vm(src, src_type, call_stack_size)? {
                Ok(vm_code) => targets::C.build_core(&vm_code.flatten()),
                Err(vm_code) => targets::C.build_std(&vm_code.flatten()),
            }
            .map_err(Error::BuildError)?,
        )?,
        // If the target is C source code, then compile the code to virtual machine code,
        // and then use the C target implementation to build the output source code.
        TargetType::X86 => write_file(
            format!("{output}.S"),
            match compile_source_to_vm(src, src_type, call_stack_size)? {
                Ok(vm_code) => targets::X86.build_core(&vm_code.flatten()),
                Err(vm_code) => targets::X86.build_std(&vm_code.flatten()),
            }
            .map_err(Error::BuildError)?,
        )?,
        // If the target is core virtual machine code, then try to compile the source to the core variant.
        // If not possible, throw an error.
        TargetType::CoreVM => match compile_source_to_vm(src, src_type, call_stack_size)? {
            Ok(vm_code) => write_file(format!("{output}.vm.lsd"), vm_code.flatten().to_string()),
            Err(_) => Err(Error::InvalidSource(
                "expected core VM program, got standard VM program".to_string(),
            )),
        }?,
        // If the target is standard virtual machine code, the compile it to virtual machine code.
        // If the result is core variant, we don't care. Just return the generated code.
        TargetType::StdVM => write_file(
            format!("{output}.vm.lsd"),
            match compile_source_to_vm(src, src_type, call_stack_size)? {
                Ok(vm_code) => vm_code.flatten().to_string(),
                Err(vm_code) => vm_code.flatten().to_string(),
            },
        )?,
        // If the target is core assembly code, then try to compile the source to the core variant.
        // If not possible, throw an error.
        TargetType::CoreASM => match compile_source_to_asm(src, src_type)? {
            Ok(asm_code) => write_file(format!("{output}.asm.lsd"), asm_code.to_string()),
            Err(_) => Err(Error::InvalidSource(
                "expected core assembly program, got standard assembly program".to_string(),
            )),
        }?,
        // If the target is standard assembly code, then try to compile the source to the standard variant.
        // If the result is core variant, we don't care. Just return the generated code.
        TargetType::StdASM => write_file(
            format!("{output}.asm.lsd"),
            match compile_source_to_asm(src, src_type)? {
                Ok(asm_code) => asm_code.to_string(),
                Err(asm_code) => asm_code.to_string(),
            },
        )?,
    }
    Ok(())
}

/// Write some contents to a file.
fn write_file(file: String, contents: String) -> Result<(), Error> {
    write(file, contents).map_err(Error::IO)
}

/// Read the contents of a file.
fn read_file(name: &str) -> Result<String, Error> {
    read_to_string(name).map_err(Error::IO)
}

fn main() -> Result<(), Error> {
    // Parse the arguments to the CLI.
    let args = Args::parse();

    // Compile the source code according to the supplied arguments.
    compile(
        read_file(&args.input)?,
        args.source_type,
        args.target_type,
        args.output,
        args.call_stack_size,
    )
}
