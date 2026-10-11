package demo;

import com.google.gson.Gson;
import org.apache.commons.lang3.StringUtils;

public class Service {
    private static final Gson GSON = new Gson();

    public static String process(String s) {
        return Helper.exclaim(GSON.toJson(StringUtils.upperCase(s)));
    }
}
