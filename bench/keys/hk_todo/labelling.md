# hk_todo labelling (zhashkevych/todo-app @ 1789ed69bd5f449bd605e23a92a1e87a83cb9fda)

Sources read: git ls-files; pkg/handler/{handler,auth,item,list,middleware,response}.go (func decls),
pkg/repository/{postgres,repository,auth_postgres,todo_list_postgres,todo_item_postgres}.go, cmd/main.go,
todo.go, user.go, server.go, schema/*.sql, go.mod, docs/swagger.yaml (paths only).
Skipped as test code: pkg/handler/*_test.go, pkg/repository/*_test.go, pkg/service/mocks/mock.go.

Router: `router := gin.New()` (handler.go:22); groups `auth := router.Group("/auth")` (26),
`api := router.Group("/api", h.userIdentity)` (32), `lists := api.Group("/lists")` (34),
`items := lists.Group(":id/items")` (42), `items := api.Group("items")` (49).
Table constants (postgres.go:9-13): usersTable="users", todoListsTable="todo_lists",
usersListsTable="users_lists", todoItemsTable="todo_items", listsItemsTable="lists_items".

## ENDPOINT

- `ENDPOINT GET /swagger/*any pkg/handler/handler.go:24` — handler.go:24 `router.GET("/swagger/*any", ginSwagger.WrapHandler(swaggerFiles.Handler))`; LIBRARY HANDLER (gin-swagger), location = registration line.
- `ENDPOINT POST /auth/sign-up pkg/handler/auth.go:22` — handler.go:28 `auth.POST("/sign-up", h.signUp)`; auth.go:22 `func (h *Handler) signUp(c *gin.Context) {`
- `ENDPOINT POST /auth/sign-in pkg/handler/auth.go:58` — handler.go:29 `auth.POST("/sign-in", h.signIn)`; auth.go:58 `func (h *Handler) signIn(c *gin.Context) {`
- `ENDPOINT POST /api/lists/ pkg/handler/list.go:24` — handler.go:36 `lists.POST("/", h.createList)`; list.go:24 `func (h *Handler) createList(`
- `ENDPOINT GET /api/lists/ pkg/handler/list.go:64` — handler.go:37 `lists.GET("/", h.getAllLists)`; list.go:64 `func (h *Handler) getAllLists(`
- `ENDPOINT GET /api/lists/:id pkg/handler/list.go:94` — handler.go:38 `lists.GET("/:id", h.getListById)`; list.go:94 `func (h *Handler) getListById(`
- `ENDPOINT PUT /api/lists/:id pkg/handler/list.go:116` — handler.go:39 `lists.PUT("/:id", h.updateList)`; list.go:116 `func (h *Handler) updateList(`
- `ENDPOINT DELETE /api/lists/:id pkg/handler/list.go:143` — handler.go:40 `lists.DELETE("/:id", h.deleteList)`; list.go:143 `func (h *Handler) deleteList(`
- `ENDPOINT POST /api/lists/:id/items/ pkg/handler/item.go:10` — handler.go:42,44 `lists.Group(":id/items")` + `items.POST("/", h.createItem)`; item.go:10 `func (h *Handler) createItem(`
- `ENDPOINT GET /api/lists/:id/items/ pkg/handler/item.go:40` — handler.go:45 `items.GET("/", h.getAllItems)`; item.go:40 `func (h *Handler) getAllItems(`
- `ENDPOINT GET /api/items/:id pkg/handler/item.go:62` — handler.go:49,51 `api.Group("items")` + `items.GET("/:id", h.getItemById)`; item.go:62 `func (h *Handler) getItemById(`
- `ENDPOINT PUT /api/items/:id pkg/handler/item.go:84` — handler.go:52 `items.PUT("/:id", h.updateItem)`; item.go:84 `func (h *Handler) updateItem(`
- `ENDPOINT DELETE /api/items/:id pkg/handler/item.go:111` — handler.go:53 `items.DELETE("/:id", h.deleteItem)`; item.go:111 `func (h *Handler) deleteItem(`

Not endpoints: `h.userIdentity` (middleware.go:15) is group middleware, not a route.

## QUERIES

auth_postgres.go
- `QUERIES users insert pkg/repository/auth_postgres.go:17` — :19 `"INSERT INTO %s (name, username, password_hash) ... ", usersTable` via `r.db.QueryRow` (:21); func at :17 `func (r *AuthPostgres) CreateUser(`
- `QUERIES users read pkg/repository/auth_postgres.go:29` — :31 `"SELECT id FROM %s WHERE username=$1 AND password_hash=$2", usersTable` via `r.db.Get` (:32); func :29 `GetUser(`

todo_list_postgres.go
- `QUERIES todo_lists insert ...:19` — :26 `"INSERT INTO %s (title, description) ...", todoListsTable` via `tx.QueryRow`; func :19 `func (r *TodoListPostgres) Create(`
- `QUERIES users_lists insert ...:19` — :33 `"INSERT INTO %s (user_id, list_id) ...", usersListsTable` via `tx.Exec` (:34)
- `QUERIES todo_lists read ...:43` / `users_lists read ...:43` — :46-47 `"SELECT ... FROM %s tl INNER JOIN %s ul ..." todoListsTable, usersListsTable` via `r.db.Select`; func :43 `GetAll(`
- `QUERIES todo_lists read ...:53` / `users_lists read ...:53` — :56-58 `SELECT ... FROM %s tl INNER JOIN %s ul` todoListsTable, usersListsTable via `r.db.Get`; func :53 `GetById(`
- `QUERIES todo_lists delete ...:64` / `users_lists read ...:64` — :65-66 `"DELETE FROM %s tl USING %s ul ..." todoListsTable, usersListsTable` via `r.db.Exec`; USING table = read; func :64 `Delete(`
- `QUERIES todo_lists update ...:72` / `users_lists read ...:72` — :94-95 `"UPDATE %s tl SET %s FROM %s ul ..." todoListsTable, setQuery, usersListsTable` via `r.db.Exec` (:101); FROM table in UPDATE = read; func :72 `Update(`

todo_item_postgres.go
- `QUERIES todo_items insert ...:18` — :25 `"INSERT INTO %s (title, description) ...", todoItemsTable` via `tx.QueryRow`; func :18 `func (r *TodoItemPostgres) Create(`
- `QUERIES lists_items insert ...:18` — :34 `"INSERT INTO %s (list_id, item_id) ...", listsItemsTable` via `tx.Exec` (:35)
- `QUERIES todo_items/lists_items/users_lists read ...:44` — :46-48 `SELECT ... FROM %s ti INNER JOIN %s li ... INNER JOIN %s ul` todoItemsTable, listsItemsTable, usersListsTable via `r.db.Select`; func :44 `GetAll(`
- `QUERIES todo_items/lists_items/users_lists read ...:56` — :58-60 same SELECT/JOIN shape via `r.db.Get`; func :56 `GetById(`
- `QUERIES todo_items delete, lists_items read, users_lists read ...:68` — :69-71 `DELETE FROM %s ti USING %s li, %s ul` todoItemsTable, listsItemsTable, usersListsTable via `r.db.Exec`; func :68 `Delete(`
- `QUERIES todo_items update, lists_items read, users_lists read ...:76` — :101-103 `UPDATE %s ti SET %s FROM %s li, %s ul` todoItemsTable, setQuery, listsItemsTable, usersListsTable via `r.db.Exec` (:106); func :76 `Update(`

Not QUERIES: schema/000001_init.up.sql (CREATE TABLE only) and .down.sql (DROP TABLE only) are DDL;
NewPostgresDB (postgres.go:25) only opens/pings; services/handlers are callers only.

## MAPS

None. go.mod has no ORM (sqlx + lib/pq only). todo.go `TodoList`, `TodoItem`, `UsersList`, `ListsItem` and
user.go `User` are plain structs with at most sqlx `db:"..."` tags; no table name is declared anywhere.

## Uncertain

1. Trailing slash on `lists.POST("/")`, `lists.GET("/")`, `items.POST("/")`, `items.GET("/")`: gin's joinPaths keeps
   the trailing slash, so the registered routes are `/api/lists/` and `/api/lists/:id/items/` (written that way in the key).
   docs/swagger.yaml:56 lists `/api/lists` without the slash; a scorer that normalizes trailing slashes should treat both as equal.
2. `GET /swagger/*any` is a library handler (ginSwagger.WrapHandler) — included at the registration line handler.go:24 as instructed.
3. Update functions (todo_list_postgres.go:72, todo_item_postgres.go:76) build the SET clause dynamically; table names are
   still constant, so they are keyed normally. FROM-joined tables in UPDATE and USING tables in DELETE are labelled `read`.
4. Group path `lists.Group(":id/items")` and `api.Group("items")` have no leading slash; gin's path.Join yields
   `/api/lists/:id/items` and `/api/items`.
