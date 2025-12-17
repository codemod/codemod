from models import User

def create_user(name):
    return User(name)

admin = User("Admin")
guest = User("Guest")



