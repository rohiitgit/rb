package acme;

import java.util.List;
import org.springframework.web.bind.annotation.RestController;

@RestController
public class UserController implements UsersApi {
  @Override
  public List<String> listUsers() {
    return List.of();
  }

  @Override
  public String getUser(String id) {
    return id;
  }
}
