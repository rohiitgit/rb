import { UsersApi } from "./gen";
import { UsersClient } from "./grpc-gen";

const usersApi = new UsersApi();

export async function names(): Promise<string[]> {
  return usersApi.listUsers();
}

export async function get(addr: string, req: unknown): Promise<string> {
  const client = new UsersClient(addr);
  return client.get(req);
}
