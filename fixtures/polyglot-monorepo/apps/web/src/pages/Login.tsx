import { login } from "../api/auth";

export function LoginForm() {
  async function onSubmit(email: string, password: string) {
    const token = await login({ email, password });
    localStorage.setItem("token", token);
  }
  return <form onSubmit={() => onSubmit("", "")} />;
}
