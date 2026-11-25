import { findUser, removeUser, updateUser } from "../../../../lib/db";

type Context = { params: Promise<{ id: string }> };

export async function GET(request: Request, { params }: Context) {
  const { id } = await params;
  const user = findUser(id);
  if (!user) return new Response("not found", { status: 404 });
  return Response.json(user);
}

export async function PATCH(request: Request, context: Context) {
  const { id } = await context.params;
  const body = await request.json();
  return Response.json(updateUser(id, body.name));
}

export const DELETE = async (_request: Request, { params }: Context) => {
  const { id } = await params;
  removeUser(id);
  return new Response(null, { status: 204 });
};
