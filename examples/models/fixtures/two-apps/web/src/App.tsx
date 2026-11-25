import { createUser, deleteUser, listUsers, loadAnything, loadTeams, renameUser } from "./users/actions";
import { UserLookup } from "./users/UserLookup";

export function App() {
  return (
    <main>
      <UserLookup />
      <button onClick={() => listUsers()}>Refresh</button>
      <button onClick={() => createUser("Ada")}>Add</button>
      <button onClick={() => renameUser("1", "Grace")}>Rename</button>
      <button onClick={() => deleteUser("1")}>Delete</button>
      <button onClick={() => loadTeams()}>Teams</button>
      <button onClick={() => loadAnything("users")}>Anything</button>
    </main>
  );
}
