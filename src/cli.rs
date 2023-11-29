//! # The Sage Compiler
//!
//! This program implements the CLI frontend to the sage programming
//! language. This can compile sage's various source languages to
//! the supported targets provided by the compiler.
use clap::*;
use sage::{
    lir::*,
    parse::*,
    targets::{self, CompiledTarget},
    vm::*,
    LOGO_WITH_COLOR, *,
};
use std::{
    fmt,
    fs::{read_to_string, write},
};

use log::error;

// The stack sizes of the threads used to compile the code.
const RELEASE_STACK_SIZE_MB: usize = 512;
const DEBUG_STACK_SIZE_MB: usize = RELEASE_STACK_SIZE_MB;

#[derive(clap::ValueEnum, Default, Clone, Debug, PartialEq)]
enum LogLevel {
    /// Print all the errors
    Error,
    /// Print all the warnings and errors
    Warn,
    /// Print all the info messages
    Info,
    /// Print all the debug information
    Debug,
    /// Trace the compilation of the program
    Trace,
    /// Display no messages
    #[default]
    Off,
}

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
    /// Compile to My OS source code (GCC only).
    MyOS,
    /// Compile to C source code (GCC only).
    C,
    /// Compile to x86 assembly code.
    X86,
}

/// The source language options to compile.
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum SourceType {
    /// Compile Sage Frontend code.
    Sage,
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
#[clap(author, version, about = Some(LOGO_WITH_COLOR), long_about = Some(LOGO_WITH_COLOR), max_term_width=90)]
struct Args {
    /// The input file to compiler.
    #[clap(value_parser)]
    input: String,

    /// The file to write the output of the compiler to.
    #[clap(short, long, value_parser, default_value = "out")]
    output: String,

    /// The source language to compile.
    #[clap(short, value_parser, default_value = "sage")]
    source_type: SourceType,

    /// The target language to compile to.
    #[clap(short, value_parser, default_value = "run")]
    target_type: TargetType,

    /// The number of cells allocated for the call stack.
    #[clap(short, long, value_parser, default_value = "8192")]
    call_stack_size: usize,

    /// The log level to use.
    #[clap(short, long, value_parser, default_value = "off")]
    log_level: LogLevel,

    /// The symbol to debug (if any exists). This will
    /// also enable debug logging.
    #[clap(short, long, value_parser)]
    debug: Option<String>,
}

/// The types of errors returned by the CLI.
enum Error {
    /// With the given source code location and the source code itself.
    WithSourceCode {
        loc: SourceCodeLocation,
        source_code: String,
        err: Box<Self>,
    },
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

impl Error {
    pub fn annotate_with_source(self, code: &str) -> Self {
        match self {
            Self::LirError(lir::Error::Annotated(ref err, ref metadata)) => {
                if let Some(loc) = metadata.location().cloned() {
                    Self::WithSourceCode {
                        loc,
                        source_code: code.to_owned(),
                        err: Box::new(Error::LirError(*err.clone())),
                    }
                } else {
                    self
                }
            }
            _ => self,
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::IO(e) => write!(f, "IO error: {:?}", e),
            Error::Parse(e) => write!(f, "Parse error: {}", e),
            Error::AsmError(e) => write!(f, "Assembly error: {:?}", e),
            Error::LirError(e) => write!(f, "LIR error: {}", e),
            Error::WithSourceCode {
                loc,
                source_code,
                err,
            } => {
                // use codespan_reporting::files::SimpleFiles;
                use codespan_reporting::diagnostic::{Diagnostic, Label};
                use codespan_reporting::files::SimpleFiles;
                use codespan_reporting::term::{
                    emit,
                    termcolor::{ColorChoice, StandardStream},
                };
                use no_comment::{languages, IntoWithoutComments};

                let SourceCodeLocation {
                    line,
                    column,
                    filename,
                    offset,
                    length,
                } = loc;

                let mut files = SimpleFiles::new();

                let source_code = source_code
                    .to_string()
                    .chars()
                    .without_comments(languages::rust())
                    .collect::<String>();

                let file_id = files.add(
                    filename.clone().unwrap_or("unknown".to_string()),
                    source_code,
                );
                match filename {
                    Some(filename) => {
                        let loc = format!("{}:{}:{}:{}", filename, line, column, offset);
                        // let code = format!("{}\n{}^", code, " ".repeat(*column - 1));
                        // write!(f, "Error at {}:\n{}\n{:?}", loc, code, err)?
                        let diagnostic = Diagnostic::error()
                            .with_message(format!("Error at {}", loc))
                            .with_labels(vec![Label::primary(
                                file_id,
                                *offset..*offset + length.unwrap_or(0),
                            )
                            .with_message(format!("{err:?}"))]);

                        let writer = StandardStream::stderr(ColorChoice::Always);
                        let config = codespan_reporting::term::Config::default();

                        emit(&mut writer.lock(), &config, &files, &diagnostic).unwrap();
                    }
                    None => {
                        let loc = format!("unknown:{}:{}:{}", line, column, offset);
                        // let code = format!("{}\n{}^", code, " ".repeat(*column - 1));
                        // write!(f, "Error at {}:\n{}\n{:?}", loc, code, err)?
                        let diagnostic = Diagnostic::error()
                            .with_message(format!("Error at {}", loc))
                            .with_labels(vec![Label::primary(
                                file_id,
                                *offset..*offset + length.unwrap_or(0),
                            )
                            .with_message(format!("{err:?}"))]);

                        let writer = StandardStream::stderr(ColorChoice::Always);
                        let config = codespan_reporting::term::Config::default();

                        emit(&mut writer.lock(), &config, &files, &diagnostic).unwrap();
                    }
                }
                Ok(())
                // let loc = format!("{}:{}:{}:{}", filename, line, column, offset);

                // write!(f, "Error at {}:\n{}\n{:?}", loc, code, err)
            }
            Error::InterpreterError(e) => write!(f, "Interpreter error: {}", e),
            Error::BuildError(e) => write!(f, "Build error: {}", e),
            Error::InvalidSource(e) => write!(f, "Invalid source: {}", e),
        }
    }
}

/// Compile a given source language to virtual machine code.
fn compile_source_to_vm(
    filename: Option<&str>,
    src: String,
    src_type: SourceType,
    call_stack_size: usize,
) -> Result<Result<sage::vm::CoreProgram, sage::vm::StandardProgram>, Error> {
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
        SourceType::Sage => {
            match parse_frontend(&src, filename)
                .map_err(Error::Parse)?
                .compile()
                .map_err(Error::LirError)
                .map_err(|e| e.annotate_with_source(&src))?
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
    filename: Option<&str>,
    src: String,
    src_type: SourceType,
) -> Result<Result<sage::asm::CoreProgram, sage::asm::StandardProgram>, Error> {
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

        // If the source language is Sage, parse it and compile it to assembly code.
        SourceType::Sage => parse_frontend(&src, filename)
            .map_err(Error::Parse)?
            .compile()
            .map_err(Error::LirError)
            .map_err(|e| e.annotate_with_source(&src)),
        // If the source language is a virtual machine program,
        // then we cannot compile it to assembly. Throw an error.
        SourceType::CoreVM | SourceType::StdVM => Err(Error::InvalidSource(
            "cannot compile a core VM program to assembly".to_string(),
        )),
    }
}

/// Compile code in a given source language to a given target language.
fn compile(
    filename: Option<&str>,
    src: String,
    src_type: SourceType,
    target: TargetType,
    output: String,
    call_stack_size: usize,
    debug: bool,
) -> Result<(), Error> {
    match target {
        // If the target is `Run`, then compile the code and execute it with the interpreter.
        TargetType::Run => match compile_source_to_vm(filename, src, src_type, call_stack_size)? {
            // If the code is core variant virtual machine code
            Ok(vm_code) => {
                CoreInterpreter::new(StandardDevice::default())
                    .run(&vm_code)
                    .map_err(Error::InterpreterError)?;
            }
            // If the code is standard variant virtual machine code
            Err(vm_code) => {
                StandardInterpreter::new(StandardDevice::default())
                    .run(&vm_code)
                    .map_err(Error::InterpreterError)?;
            }
        },
        // If the target is MyOS source code, then compile the code to virtual machine code,
        // and then use the MyOS target implementation to build the output source code.
        TargetType::MyOS => write_file(
            format!("{output}.c"),
            match compile_source_to_vm(filename, src, src_type, call_stack_size)? {
                Ok(vm_code) => targets::MyOS::default().build_core(&vm_code.flatten()),
                Err(vm_code) => targets::MyOS::default().build_std(&vm_code.flatten()),
            }
            .map_err(Error::BuildError)?,
        )?,
        // If the target is C source code, then compile the code to virtual machine code,
        // and then use the C target implementation to build the output source code.
        TargetType::C => write_file(
            format!("{output}.c"),
            match compile_source_to_vm(filename, src, src_type, call_stack_size)? {
                Ok(vm_code) => targets::C::default().build_core(&vm_code.flatten()),
                Err(vm_code) => targets::C::default().build_std(&vm_code.flatten()),
            }
            .map_err(Error::BuildError)?,
        )?,
        // If the target is x86 assembly code, then compile the code to virtual machine code,
        // and then use the x86 target implementation to build the output source code.
        TargetType::X86 => write_file(
            format!("{output}.s"),
            match compile_source_to_vm(filename, src, src_type, call_stack_size)? {
                Ok(vm_code) => targets::X86::default().build_core(&vm_code.flatten()),
                Err(vm_code) => targets::X86::default().build_std(&vm_code.flatten()),
            }
            .map_err(Error::BuildError)?,
        )?,
        // If the target is core virtual machine code, then try to compile the source to the core variant.
        // If not possible, throw an error.
        TargetType::CoreVM => match compile_source_to_vm(filename, src, src_type, call_stack_size)?
        {
            Ok(vm_code) if debug => write_file(
                format!("{output}.vm.sg"),
                format!("{:#}", vm_code.flatten()),
            ),
            Ok(vm_code) => write_file(format!("{output}.vm.sg"), vm_code.flatten().to_string()),
            Err(_) => Err(Error::InvalidSource(
                "expected core VM program, got standard VM program".to_string(),
            )),
        }?,
        // If the target is standard virtual machine code, the compile it to virtual machine code.
        // If the result is core variant, we don't care. Just return the generated code.
        TargetType::StdVM => write_file(
            format!("{output}.vm.sg"),
            match compile_source_to_vm(filename, src, src_type, call_stack_size)? {
                Ok(vm_code) if debug => format!("{:#}", vm_code.flatten()),
                Err(vm_code) if debug => format!("{:#}", vm_code.flatten()),
                Ok(vm_code) => vm_code.flatten().to_string(),
                Err(vm_code) => vm_code.flatten().to_string(),
            },
        )?,
        // If the target is core assembly code, then try to compile the source to the core variant.
        // If not possible, throw an error.
        TargetType::CoreASM => match compile_source_to_asm(filename, src, src_type)? {
            Ok(asm_code) if debug => {
                write_file(format!("{output}.asm.sg"), format!("{:#}", asm_code))
            }
            Ok(asm_code) => write_file(format!("{output}.asm.sg"), asm_code.to_string()),
            Err(_) => Err(Error::InvalidSource(
                "expected core assembly program, got standard assembly program".to_string(),
            )),
        }?,
        // If the target is standard assembly code, then try to compile the source to the standard variant.
        // If the result is core variant, we don't care. Just return the generated code.
        TargetType::StdASM => write_file(
            format!("{output}.asm.sg"),
            match compile_source_to_asm(filename, src, src_type)? {
                Ok(core_asm_code) if debug => format!("{:#}", core_asm_code),
                Err(std_asm_code) if debug => format!("{:#}", std_asm_code),
                Ok(core_asm_code) => core_asm_code.to_string(),
                Err(std_asm_code) => std_asm_code.to_string(),
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

/// Run the CLI.
fn cli() {
    // Parse the arguments to the CLI.
    let args = Args::parse();
    let mut builder = env_logger::Builder::from_default_env();
    builder.format_timestamp(None);

    let target = args.debug.as_ref().map(|s| s.as_str());

    // Set the log level.
    match args.log_level {
        LogLevel::Error if !args.debug.is_some() => builder.filter(target, log::LevelFilter::Error),
        LogLevel::Warn if !args.debug.is_some() => builder.filter(target, log::LevelFilter::Warn),
        LogLevel::Off if !args.debug.is_some() => builder.filter(target, log::LevelFilter::Error),
        LogLevel::Info if !args.debug.is_some() => builder.filter(target, log::LevelFilter::Info),
        LogLevel::Trace => builder.filter(target, log::LevelFilter::Trace),
        _ => builder.filter(target, log::LevelFilter::Debug),
    };

    builder.init();

    match read_file(&args.input) {
        Ok(file_contents) => {
            match compile(
                Some(&args.input),
                file_contents,
                args.source_type,
                args.target_type,
                args.output,
                args.call_stack_size,
                args.debug.is_some(),
            ) {
                Ok(_) => {}
                Err(e) => {
                    error!("{e:#?}");
                }
            }
        }
        Err(e) => {
            error!("Error reading file: {e:?}");
        }
    }
}

fn main() {
    // If we're in debug mode, start the compilation in a separate thread.
    // This is to allow the process to have more stack space.
    if !cfg!(debug_assertions) {
        let child = std::thread::Builder::new()
            .stack_size(RELEASE_STACK_SIZE_MB * 1024 * 1024)
            .spawn(cli)
            .unwrap();

        // Wait for the thread to finish.
        child.join().unwrap()
    } else {
        let child = std::thread::Builder::new()
            .stack_size(DEBUG_STACK_SIZE_MB * 1024 * 1024)
            .spawn(cli)
            .unwrap();

        // Wait for the thread to finish.
        child.join().unwrap()
    }
}
