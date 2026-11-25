import { Menu } from "./Menu";

export function Nav(props) {
  return (
    <ul>
      <Menu.Item label="Home" />
      <props.Slot />
    </ul>
  );
}
