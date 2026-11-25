import { loadUser } from "./db";
import { publicProcedure, router } from "./trpc";

export const userRouter = router({
  getUser: publicProcedure.query(() => loadUser()),
});
