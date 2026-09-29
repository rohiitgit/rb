// A same-named method in an unrelated class: the login route must not be linked to it.
export class Cache {
  findUserByEmail(email: string) {
    return undefined;
  }
}
