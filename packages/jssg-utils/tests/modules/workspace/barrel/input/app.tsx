import { Button, Modal, formatValue, VERSION } from "./ui";
import type { Props } from "./ui/index";

export const view = (props: Props) => [Button, Modal, formatValue, VERSION, props];
