package demo;

public class Main {
    public static String run(String s) {
        return Service.process(s);
    }

    public static void main(String[] args) {
        System.out.println(run("hi"));
    }
}
