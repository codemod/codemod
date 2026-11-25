import { showUser } from "./show";

export function UserLookup() {
  async function onSubmit(event) {
    event.preventDefault();
    const form = event.currentTarget;
    const id = form.elements.userId.value;
    const res = await fetch(`/api/users/${id}`);
    const user = await res.json();
    showUser(user);
  }

  return (
    <form onSubmit={onSubmit}>
      <input name="userId" />
      <button type="submit">Find</button>
    </form>
  );
}
