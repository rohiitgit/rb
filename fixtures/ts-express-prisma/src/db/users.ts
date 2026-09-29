import { Pool } from "pg";
import { PrismaClient } from "@prisma/client";

const pool = new Pool();
const prisma = new PrismaClient();

export async function findUserByEmail(email: string) {
  return prisma.user.findUnique({ where: { email } });
}

export async function createSession(userId: number) {
  const { rows } = await pool.query(
    `INSERT INTO sessions (user_id, token) VALUES ($1, gen_random_uuid()) RETURNING token`,
    [userId],
  );
  return rows[0];
}
