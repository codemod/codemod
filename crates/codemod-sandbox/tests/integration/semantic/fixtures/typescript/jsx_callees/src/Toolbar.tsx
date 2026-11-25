import { toast } from "sonner";
import { Tooltip } from "./Tooltip";
import { TrackedButton } from "./TrackedButton";
import { statusLabel } from "./status";
import { analytics } from "./analytics";

const SAVE_LABEL = "Save changes";
const SAVED_MESSAGE = "Your changes were saved";

export function Toolbar({ order, items, user }) {
  const greeting = "Signed in as " + user.name;
  const debugTag = "Toolbar rendered";
  console.log(debugTag);
  analytics.track("Toolbar Viewed");
  const isBulk = items.length > 1;

  function onSave() {
    toast.success(SAVED_MESSAGE);
  }

  return (
    <div>
      <header>{greeting}</header>
      <TrackedButton eventLabel="Toolbar Save Clicked" onClick={onSave}>
        {SAVE_LABEL}
      </TrackedButton>
      <Tooltip content="Saves all open documents">?</Tooltip>
      <span>{statusLabel(order.status)}</span>
      <p>{isBulk ? "Items" : "Item"} added to the order</p>
    </div>
  );
}
