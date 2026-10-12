import { Injectable } from '@nestjs/common';

// Decoy (contract M6j S3): a bean literally named `DB` must NOT satisfy the `'DB'`
// string token (custom tokens resolve only through explicit qualifiers, which stay
// unread). If token matching ever widens to names, the service point resolves and the
// guards catch it.
@Injectable()
export class DB {
  open(): string {
    return 'open';
  }
}
