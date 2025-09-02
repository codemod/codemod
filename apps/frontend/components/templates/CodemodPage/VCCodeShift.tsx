import Icon from "@/components/shared/Icon";
import RunCTAButton from "@/components/shared/RunCTAButton";
import { CURSOR_PREFIX, VSCODE_PREFIX } from "@/constants";

import type { CodemodPagePayload } from "@/types";
import type { AutomationResponseVersion } from "@/types/object.types";
export const VCCodeShift = (
  data: CodemodPagePayload & { currentVersion: AutomationResponseVersion },
) => (
  <div className="flex flex-col gap-xs">
    <p className="body-s">IDE extension</p>
    <RunCTAButton
      href={data.currentVersion.vsCodeLink}
      title={data.globalLabels?.vsCodeExtensionButtonLabel || "Run in VS Code"}
      toastMessage="Opening Visual Studio Code..."
      toastOptions={{
        icon: <Icon name="vscode" className="h-5 w-5" />,
        className: "flex items-center gap-xs",
      }}
    />
    <RunCTAButton
      href={data.currentVersion.vsCodeLink.replace(
        VSCODE_PREFIX,
        CURSOR_PREFIX,
      )}
      title={"Run in Cursor"}
      toastMessage="Opening Cursor..."
      toastOptions={{
        icon: (
          <img
            src="/icons/cursor-ide.svg"
            width={30}
            height={30}
            alt="cursor-ide-svg"
            style={{ marginLeft: "0.2rem" }}
          />
        ),
        className: "flex items-center gap-xs",
      }}
    />
  </div>
);
