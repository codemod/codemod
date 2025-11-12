// Types for GitHub API (used by mirage mocks)
export interface GithubCommit {
  sha: string;
  url: string;
}

export interface GithubRequiredStatusChecks {
  enforcement_level: string;
  contexts: string[];
}

export type GHBranch = Readonly<{
  name: string;
  commit: GithubCommit;
  protected: boolean;
  protection: {
    enabled: boolean;
    required_status_checks: GithubRequiredStatusChecks;
  };
}>;

export type GithubRepository = {
  id: number;
  name: string;
  full_name: string;
  private: boolean;
  html_url: string;
  default_branch: string;
  permissions: {
    admin: boolean;
    push: boolean;
    pull: boolean;
  };
};

export type Result =
  | {
      status: "progress" | "error";
      message: string;
    }
  | {
      status: "executing codemod";
      progress: { processed: number; total: number };
    }
  | {
      status: "done";
      link: string;
    };
