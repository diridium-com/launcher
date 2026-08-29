import java.awt.AWTEvent;
import java.awt.Image;
import java.awt.Taskbar;
import java.awt.Toolkit;
import java.awt.Window;
import java.awt.event.WindowEvent;
import java.io.File;
import java.lang.reflect.Method;
import javax.imageio.ImageIO;

/**
 * Feasibility spike for launcher issue #17: set the Dock/taskbar icon of the
 * launched admin without touching its jars. Prepend this class's jar/dir to
 * the admin classpath and swap the main class:
 *
 *   java -Dlauncher.icon=/path/to/icon.png -Dlauncher.name="My Server" \
 *        -Dlauncher.main=com.mirth.connect.client.ui.Mirth \
 *        -cp <bootstrap.jar>:<admin jars...> IconBootstrap <admin args...>
 *
 * Where the platform supports Taskbar.ICON_IMAGE (macOS Dock, some Linux DEs)
 * the icon is set directly. Otherwise (Windows) an AWT listener stamps every
 * AWT window as it opens; JavaFX stages are NOT AWT windows, so whether this
 * reaches a JavaFX admin's taskbar entry is exactly what the spike measures.
 * Icon failures never block the launch.
 */
public class IconBootstrap {
    public static void main(String[] args) throws Exception {
        String icon = System.getProperty("launcher.icon");
        String mainClass = System.getProperty("launcher.main");
        String name = System.getProperty("launcher.name");

        // FIRST, before anything can touch AWT: macOS reads this only during
        // AWT initialization, so setting it later is silently ignored. It
        // names the application menu, which would otherwise read "java".
        // Ignored off macOS.
        //
        // It does NOT change the Dock icon's hover tooltip, and neither does
        // the -Xdock:name launcher flag (measured 2026-08-29; the JDK turns
        // -Xdock:name into this same property). The tooltip comes from
        // LaunchServices, which falls back to the executable filename for a
        // process with no app bundle. Unreachable from inside the JVM -- do
        // not re-try -Xdock:name for it.
        if (name != null && !name.trim().isEmpty()) {
            System.setProperty("apple.awt.application.name", name.trim());
        }

        // X11 (GNOME especially) picks a window's icon by matching WM_CLASS
        // against an installed .desktop file, ignoring the icon the window
        // sets on itself. WM_CLASS defaults to the main class, so every admin
        // looked identical. Give each connection its own, to pair with the
        // .desktop entry the launcher writes.
        //
        // Must run AFTER getDefaultToolkit() (XToolkit.init sets this field
        // itself) and BEFORE the first window is created. Off X11 the field
        // does not exist and this quietly does nothing.
        String wmClass = System.getProperty("launcher.wmclass");
        if (wmClass != null && !wmClass.trim().isEmpty()) {
            try {
                Toolkit tk = Toolkit.getDefaultToolkit();
                java.lang.reflect.Field f = tk.getClass().getDeclaredField("awtAppClassName");
                f.setAccessible(true);
                f.set(tk, wmClass.trim());
            } catch (Throwable t) {
                System.out.println("[IconBootstrap] WM_CLASS not set (" + t + ")");
            }
        }

        if (mainClass == null || mainClass.isEmpty()) {
            System.err.println("[IconBootstrap] -Dlauncher.main is required");
            System.exit(2);
        }
        try {
            Image img = ImageIO.read(new File(icon));
            Taskbar tb = Taskbar.isTaskbarSupported() ? Taskbar.getTaskbar() : null;
            if (tb != null && tb.isSupported(Taskbar.Feature.ICON_IMAGE)) {
                tb.setIconImage(img);
                System.out.println("[IconBootstrap] taskbar icon set directly");
            } else {
                Toolkit.getDefaultToolkit().addAWTEventListener(e -> {
                    if (e.getID() == WindowEvent.WINDOW_OPENED && e.getSource() instanceof Window) {
                        ((Window) e.getSource()).setIconImage(img);
                    }
                }, AWTEvent.WINDOW_EVENT_MASK);
                System.out.println("[IconBootstrap] ICON_IMAGE unsupported; window-stamping listener installed");
            }
        } catch (Throwable t) {
            System.err.println("[IconBootstrap] icon setup failed, launching anyway: " + t);
        }
        Method m = Class.forName(mainClass).getMethod("main", String[].class);
        m.invoke(null, (Object) args);
    }
}
