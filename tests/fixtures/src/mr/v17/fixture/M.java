package fixture;

public class M {
    record Named(String value) {}

    public String name() {
        return new Named("v17").value();
    }
}
