declare module "codemod:files" {
  /** The workflow callback or JSSG transform's view of its target. */
  export interface Files {
    /** Read a UTF-8 file, including pending changes. Throws if the file is missing. */
    read(path: string): string;
    /** Whether a path names an existing file or directory. */
    exists(path: string): boolean;
    /** Matching file paths, relative to the target, sorted by path. */
    list(glob: string): string[];
    /** Stage a new UTF-8 file. Throws if the path already exists. */
    create(path: string, text: string): void;
  }

  /** All paths must stay inside the target. */
  export const files: Readonly<Files>;
}
