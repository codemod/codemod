import { allUsers, insertUser } from "../../../lib/db";

export async function GET() {
  return Response.json(allUsers());
}

export async function POST(request: Request) {
  const body = await request.json();
  return Response.json(insertUser(body.name), { status: 201 });
}
