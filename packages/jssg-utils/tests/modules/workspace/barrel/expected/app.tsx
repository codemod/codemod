import { VERSION } from "./ui";
import { Button } from "./ui/button";
import Modal from "./ui/modal";
import { formatValue } from "./ui/format";
import type { Props } from "./ui/types";

export const view = (props: Props) => [Button, Modal, formatValue, VERSION, props];
