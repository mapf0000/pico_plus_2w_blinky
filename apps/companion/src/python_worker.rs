//! Child entry point. No BLE, egui, profile, host agent or environment access.
use python_runtime::{
    PythonProcess, StepResult,
    ipc::{self, Command, Input, Response},
};
use std::io;
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    crate::python_heap::arm();
    let mut reader = io::stdin().lock();
    let mut writer = io::stdout().lock();
    let mut process = None;
    let mut run = 0;
    let mut step = 0u64;
    loop {
        let command: Command = serde_json::from_slice(&ipc::read_frame(&mut reader)?)?;
        if command.version != ipc::VERSION
            || command.run == 0
            || command.step != step.checked_add(1).ok_or("Python step exhausted")?
        {
            return Err("Invalid Python command correlation".into());
        }
        let result = match command.input {
            Input::Start { source } if process.is_none() && step == 0 => {
                run = command.run;
                let mut vm = PythonProcess::new()?;
                let result = vm.start(&source);
                process = Some(vm);
                result
            }
            Input::Resume { value } if command.run == run => process
                .as_mut()
                .ok_or("Python not started")?
                .resume(&serde_json::to_string(&value)?),
            Input::Raise { code, message } if command.run == run => process
                .as_mut()
                .ok_or("Python not started")?
                .raise(&code, &message),
            _ => return Err("Invalid Python command order".into()),
        };
        step = command.step;
        let result: StepResult = serde_json::from_str(&result)?;
        let terminal = !matches!(result, StepResult::Effect { .. });
        ipc::write_frame(
            &mut writer,
            &Response {
                version: ipc::VERSION,
                run,
                step,
                heap_peak: crate::python_heap::peak(),
                result,
            },
        )?;
        if terminal {
            return Ok(());
        }
    }
}
