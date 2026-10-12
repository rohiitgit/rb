import { Controller, Get } from '@nestjs/common';
import { ShopService } from './shop.service';

@Controller('shop')
export class ShopController {
  constructor(private readonly svc: ShopService) {}

  @Get()
  list(): string {
    return this.svc.find();
  }
}
