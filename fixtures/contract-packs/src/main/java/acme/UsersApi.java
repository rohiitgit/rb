package acme;

import java.util.List;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.RequestMapping;

@RequestMapping("/users")
public interface UsersApi {
  @GetMapping
  List<String> listUsers();

  @GetMapping("/{id}")
  String getUser(@PathVariable String id);
}
