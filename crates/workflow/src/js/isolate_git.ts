// `codemod/git` inside steps and callbacks, for the files they import. Git
// steps only start from the workflow body, so using one here says so;
// importing one still works, for helpers the body and callbacks share.

function bodyOnly(name: string): () => never {
  return () => {
    throw new Error(
      `${name}() can only be called in the workflow body, not inside a step or forEachFile callback`,
    );
  };
}

export const branch = bodyOnly("branch");
export const commit = bodyOnly("commit");
export const pullRequest = bodyOnly("pullRequest");
