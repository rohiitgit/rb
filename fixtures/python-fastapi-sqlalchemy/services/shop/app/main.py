from fastapi import APIRouter, FastAPI

from app.models import Order, User

from .store import active_users, order_total

router = APIRouter(prefix="/api")
v1 = APIRouter()


@router.get("/users")
def list_users():
    return active_users()


@v1.get("/users")
def list_users_v1():
    return active_users()


@router.post("/orders")
def create_order():
    return order_total()


app = FastAPI()


@app.get("/health")
def health():
    return {"ok": True}


@app.get("/owner/{name}")
def owner(name: str):
    return {"user_table": User.__tablename__, "order_table": Order.__tablename__}


app.include_router(router)
app.include_router(v1, prefix="/v2")
