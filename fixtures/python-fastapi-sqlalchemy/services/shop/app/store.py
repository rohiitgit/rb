from app.models import User


def active_users():
    return ["ada"]


def order_total():
    return User.__tablename__


def user_emails(db, active):
    return db.execute(f"SELECT email FROM users WHERE active = {active}")


def order_ids(db):
    return db.execute("SELECT id FROM orders")
