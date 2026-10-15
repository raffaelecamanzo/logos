export async function request<T>(path: string): Promise<T> {
  const res = await fetch(path);
  return (await res.json()) as T;
}
