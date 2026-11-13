// `codemod/llm` is not available when Node.js runs the workflow yet.

export function generate() {
  return Promise.reject(
    new Error(
      "codemod/llm is not available when Node.js runs the workflow; run it with the codemod CLI",
    ),
  );
}

export default generate;
