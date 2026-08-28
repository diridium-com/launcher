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
 *   java -Dlauncher.icon=/path/to/icon.png -Dlauncher.main=com.mirth.connect.client.ui.Mirth \
 *        -cp <spike-dir>:<admin jars...> IconBootstrap <admin args...>
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
