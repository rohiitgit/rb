# spring_boot_microservices: labelling notes

Pinned at 5899a53015228692410b8120df82924e665a5d70. Paths are relative to the repository. None of the five services sets `server.servlet.context-path` or `server.contextPath` (checked: `api-gateway/src/main/resources/application.yml`, `application.properties.bak`, and every service's `application.properties`), so no context-path prefix is added.

Abbreviations used below:
- `W` = `bookstore-webapp/src/main/java/com/sivalabs/bookstore/webapp/web/controllers/`
- `C` = `catalog-service/src/main/java/com/sivalabs/bookstore/catalog/`
- `O` = `order-service/src/main/java/com/sivalabs/bookstore/orders/`
- `N` = `notification-service/src/main/java/com/sivalabs/bookstore/notifications/`

## ENDPOINT

### bookstore-webapp (`@Controller`, no class-level `@RequestMapping`)
- `ENDPOINT GET / W/ProductController.java:24`: line 23 has a bare `@GetMapping`, and line 24 is `String index()`. With no path and no class prefix, the route is `/`.
- `ENDPOINT GET /products W/ProductController.java:29`: line 28 `@GetMapping("/products")`, line 29 `String showProductsPage(...)`.
- `ENDPOINT GET /api/products W/ProductController.java:36`: line 34 `@GetMapping("/api/products")`, line 36 `PagedResult<Product> products(...)`.
- `ENDPOINT GET /cart W/OrderController.java:34`: line 33 `@GetMapping("/cart")`, line 34 `String cart()`.
- `ENDPOINT POST /api/orders W/OrderController.java:40`: line 38 `@PostMapping("/api/orders")`, line 40 `OrderConfirmationDTO createOrder(...)`.
- `ENDPOINT GET /orders/{orderNumber} W/OrderController.java:46`: line 45 `@GetMapping("/orders/{orderNumber}")`, line 46 `String showOrderDetails(...)`.
- `ENDPOINT GET /api/orders/{orderNumber} W/OrderController.java:53`: line 51 `@GetMapping("/api/orders/{orderNumber}")`, line 53 `OrderDTO getOrder(...)`.
- `ENDPOINT GET /orders W/OrderController.java:59`: line 58 `@GetMapping("/orders")`, line 59 `String showOrders()`.
- `ENDPOINT GET /api/orders W/OrderController.java:65`: line 63 `@GetMapping("/api/orders")`, line 65 `List<OrderSummary> getOrders()`.

### catalog-service (`C/web/controllers/ProductController.java`, line 17 `@RequestMapping("/api/products")`)
- `ENDPOINT GET /api/products C/web/controllers/ProductController.java:27`: line 26 bare `@GetMapping`, line 27 `PagedResult<Product> getProducts(...)`.
- `ENDPOINT GET /api/products/{code} C/web/controllers/ProductController.java:33`: line 32 `@GetMapping("/{code}")`, line 33 `ResponseEntity<Product> getProductByCode(...)`.

### order-service (`O/web/controllers/OrderController.java`, line 25 `@RequestMapping("/api/orders")`)
- `ENDPOINT POST /api/orders O/web/controllers/OrderController.java:40`: line 38 bare `@PostMapping`, line 40 `CreateOrderResponse createOrder(...)`.
- `ENDPOINT GET /api/orders O/web/controllers/OrderController.java:47`: line 46 bare `@GetMapping`, line 47 `List<OrderSummary> getOrders()`.
- `ENDPOINT GET /api/orders/{orderNumber} O/web/controllers/OrderController.java:54`: line 53 `@GetMapping(value = "/{orderNumber}")`, line 54 `OrderDTO getOrder(...)`.

### Not endpoints
- **api-gateway:** its routes `/catalog/**`, `/orders/**` and `/v3/api-docs/**` are declared only in `application.yml` lines 27-44, so they forward requests rather than serve them. `SwaggerConfig.java` is a `@PostConstruct` config class, not a handler.
- **notification-service:** has no web controllers. `OrderEventHandler` holds RabbitMQ listeners, not HTTP handlers.
- **`GlobalExceptionHandler` classes:** these are exception advice, not routes.
- **`WebMvcConfig.addCorsMappings("/api/**")`:** this is CORS configuration, not a route.

## QUERIES
- `QUERIES products read C/domain/ProductRepository.java:7`: the derived query `Optional<ProductEntity> findByCode(String code);` sits on `JpaRepository<ProductEntity, Long>`, and `ProductEntity` maps `@Table(name = "products")`.
- `QUERIES products insert catalog-service/src/main/resources/db/migration/V2__add_books_data.sql:0`: line 1 reads `insert into products(code, name, description, image_url, price) values`. This is the only DML in a main `.sql` file. The V1 migrations, which contain only `create sequence`/`create table`, are DDL and are excluded.
- `QUERIES order_events read N/domain/OrderEventRepository.java:6`: the derived query `boolean existsByEventId(String eventId);` sits on `JpaRepository<OrderEventEntity, Long>`, which maps `@Table(name = "order_events")`.
- `QUERIES orders read O/domain/OrderRepository.java:11`: the derived query `List<OrderEntity> findByStatus(OrderStatus status);`, on an entity that maps `@Table(name = "orders")`.
- `QUERIES orders read O/domain/OrderRepository.java:13`: the derived query `Optional<OrderEntity> findByOrderNumber(String orderNumber);`.
- `QUERIES orders read O/domain/OrderRepository.java:26`: lines 21-25 `@Query("""select new ...OrderSummary(o.orderNumber, o.status) from OrderEntity o where o.userName = :userName""")` on line 26 `findByUserName`.
- `QUERIES orders read O/domain/OrderRepository.java:33`: lines 28-32 `@Query("""select distinct o from OrderEntity o left join fetch o.items where ...""")` on line 33 `findByUserNameAndOrderNumber`.
- `QUERIES order_items read O/domain/OrderRepository.java:33`: the same JPQL's `left join fetch o.items` joins the `items` association, which is `OrderItemEntity` → `@Table(name = "order_items")`. **Uncertain** (see below).

### Not queries
- **`OrderRepository.updateOrderStatus` (line 15):** a `default` method that only calls `findByOrderNumber` and `save` and states no SQL of its own.
- **`OrderEventRepository` in order-service (line 5):** declares no methods (`{}`).
- **Callers:** `OrderService` and `OrderEventService` (`save`, `findAll(sort)`, `delete`), `ProductService` (`findAll(pageable)`, `findByCode`) and `OrderEventHandler` (`existsByEventId`, `save`) only call queries or inherited CRUD methods, so they get no lines of their own.
- **`O/config/SchedulerConfig.java:15-16`:** `JdbcTemplateLockProvider` with `new JdbcTemplate(dataSource)`. ShedLock issues the SQL on the `shedlock` table inside the library, and the source writes no SQL string.
- **Test SQL:** `catalog-service/src/test/resources/test-data.sql` and `order-service/src/test/resources/test-orders.sql` are skipped because they are test code.

## MAPS
- `MAPS products C/domain/ProductEntity.java:17`: lines 15-16 `@Entity @Table(name = "products")`, line 17 `class ProductEntity {`.
- `MAPS order_events N/domain/OrderEventEntity.java:14`: lines 12-13 `@Entity @Table(name = "order_events")`, line 14 `public class OrderEventEntity {`.
- `MAPS orders O/domain/OrderEntity.java:25`: lines 23-24 `@Entity @Table(name = "orders")`, line 25 `class OrderEntity {`.
- `MAPS order_items O/domain/OrderItemEntity.java:16`: lines 14-15 `@Entity @Table(name = "order_items")`, line 16 `class OrderItemEntity {`.
- `MAPS order_events O/domain/OrderEventEntity.java:17`: lines 15-16 `@Entity @Table(name = "order_events")`, line 17 `class OrderEventEntity {`.

## Uncertain
1. **`QUERIES order_items read OrderRepository.java:33`.** The JPQL names only `OrderEntity`. It reaches `order_items` through `left join fetch o.items`, so the table is touched without being named. I kept the line because the query really reads that table. A key that counts only the entity named in `from` would drop it.
2. **`ENDPOINT GET /` for the webapp's `ProductController.index` (line 23 `@GetMapping`, no path).** I read an empty path with no class prefix as `/`. Spring matches it as the root, and possibly the empty string too.
3. **`OrderRepository.findByUserName` (line 26).** The name looks like a derived query, but it carries an explicit `@Query`. Either way it is one `orders read` line, so the outcome is the same.
4. **The ShedLock `shedlock` table** (`V3__add_shedlock_table.sql` is DDL only, and the library runs the queries). It is excluded from both QUERIES and MAPS.
5. **Spring Security endpoints** (`/login/oauth2/code/bookstore-webapp`, the logout flow in the webapp's `SecurityConfig`) and the actuator/springdoc endpoints come from frameworks, not from code handlers. They are excluded.
