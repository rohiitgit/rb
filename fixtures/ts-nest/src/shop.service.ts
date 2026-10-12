import { Injectable, Inject } from '@nestjs/common';

@Injectable()
export class ShopService {
  constructor(@Inject('DB') private db: Db) {}

  find(): string {
    return 'items';
  }
}
