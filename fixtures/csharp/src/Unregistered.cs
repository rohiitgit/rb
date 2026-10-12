namespace Shop;

// Decoy (contract M6j S6): never registered, so its constructor points stay silent.
// If owner registration ever widens, its edges trip the guards.
public class Unregistered
{
    public Unregistered(IMailer greeter)
    {
    }
}
