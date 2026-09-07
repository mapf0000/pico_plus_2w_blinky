import init, { PythonProcess } from "/ui/python-runtime.js";

const PROTOCOL_VERSION = 1;
let initialized = false;
let process = null;
let processId = null;
let stepId = 0;

async function ensureRuntime() {
  if (!initialized) {
    await init("/ui/python-runtime.wasm");
    initialized = true;
  }
}

function post(type, fields = {}) {
  self.postMessage({ version: PROTOCOL_VERSION, type, ...fields });
}

function publishResult(resultText) {
  let result;
  try {
    result = JSON.parse(resultText);
  } catch (_) {
    post("internal_error", {
      process_id: processId,
      message: "RustPython returned an invalid response",
    });
    return;
  }

  if (result.status === "effect") {
    stepId += 1;
    post("effect", {
      process_id: processId,
      step_id: stepId,
      layout: result.layout,
      effect: result.effect,
    });
  } else if (result.status === "completed") {
    post("stopped", { process_id: processId, reason: "completed" });
  } else if (result.status === "error") {
    post("python_error", {
      process_id: processId,
      phase: result.phase,
      message: result.message,
    });
  } else {
    post("internal_error", {
      process_id: processId,
      message: "RustPython returned an unknown response",
    });
  }
}

self.onmessage = async (event) => {
  const message = event.data;
  if (!message || message.version !== PROTOCOL_VERSION || typeof message.type !== "string") {
    post("internal_error", { message: "unsupported Worker protocol message" });
    return;
  }

  try {
    if (message.type === "start") {
      if (process !== null || typeof message.process_id !== "string" || typeof message.source !== "string") {
        throw new Error("invalid or duplicate start message");
      }
      processId = message.process_id;
      await ensureRuntime();
      process = new PythonProcess();
      publishResult(process.start(message.source));
      return;
    }

    if (process === null || message.process_id !== processId || message.step_id !== stepId) {
      throw new Error("stale or mismatched process step");
    }
    if (message.type === "resume") {
      publishResult(process.resume(JSON.stringify(message.value ?? null)));
    } else if (message.type === "raise") {
      if (typeof message.error_code !== "string" || typeof message.message !== "string") {
        throw new Error("invalid raise message");
      }
      publishResult(process.raise(message.error_code, message.message));
    } else {
      throw new Error("unknown Worker message type");
    }
  } catch (error) {
    post("internal_error", {
      process_id: processId,
      message: error instanceof Error ? error.message : "Worker failed",
    });
  }
};
