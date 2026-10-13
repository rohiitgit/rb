namespace Other;

// Decoy (contract M6k pass 1): a different namespace with no `using` never
// resolves Shop types, so this supertype stays silent.
public class OtherDecoy : Helper
{
}
