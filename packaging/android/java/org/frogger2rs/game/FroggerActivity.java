package org.frogger2rs.game;

import android.app.NativeActivity;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.provider.OpenableColumns;
import android.util.Log;
import android.graphics.Rect;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.ArrayList;
import java.util.Collections;

/**
 * The game runs in native code (crates/frogger-android, android_main). This subclass only
 * adds what NativeActivity cannot do: immersive full screen, and the system file picker
 * (Storage Access Framework) for importing the user's own disc image. Picked documents are
 * copied into files/import/; the native import screen picks them up from there.
 */
public class FroggerActivity extends NativeActivity {
    private static final String TAG = "frogger";
    private static final int PICK = 1;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        if (Build.VERSION.SDK_INT >= 28) {
            WindowManager.LayoutParams lp = getWindow().getAttributes();
            lp.layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
            getWindow().setAttributes(lp);
        }
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        immersive();
        // Swiping in from the left edge opens the game's overlay menu: keep the system Back
        // gesture off the middle of that edge (Android allows up to 200 dp per edge).
        if (Build.VERSION.SDK_INT >= 29) {
            final View decor = getWindow().getDecorView();
            decor.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> {
                float dp = getResources().getDisplayMetrics().density;
                int band = (int) (200 * dp), edge = (int) (48 * dp), mid = (b - t) / 2;
                decor.setSystemGestureExclusionRects(
                    Collections.singletonList(new Rect(0, Math.max(0, mid - band / 2), edge, mid + band / 2)));
            });
        }
    }

    @Override
    public void onWindowFocusChanged(boolean focus) {
        super.onWindowFocusChanged(focus);
        if (focus) immersive();
    }

    @SuppressWarnings("deprecation")
    private void immersive() {
        if (Build.VERSION.SDK_INT >= 30) {
            getWindow().setDecorFitsSystemWindows(false);
            WindowInsetsController c = getWindow().getInsetsController();
            if (c != null) {
                c.hide(WindowInsets.Type.systemBars());
                c.setSystemBarsBehavior(WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
            }
        } else {
            getWindow().getDecorView().setSystemUiVisibility(
                View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY | View.SYSTEM_UI_FLAG_FULLSCREEN
                | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION | View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN);
        }
    }

    /** Called from native code (any thread): opens the system file picker. */
    public void pickDiscImage() {
        runOnUiThread(() -> {
            Intent i = new Intent(Intent.ACTION_OPEN_DOCUMENT);
            i.addCategory(Intent.CATEGORY_OPENABLE);
            i.setType("*/*");
            i.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true);
            try {
                startActivityForResult(i, PICK);
            } catch (Exception e) {
                Log.e(TAG, "no file picker: " + e);
                status("error no file picker on this device");
            }
        });
    }

    @Override
    protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request != PICK) return;
        if (result != RESULT_OK || data == null) {
            status("cancelled");
            return;
        }
        final ArrayList<Uri> uris = new ArrayList<>();
        if (data.getClipData() != null) {
            for (int k = 0; k < data.getClipData().getItemCount(); k++) uris.add(data.getClipData().getItemAt(k).getUri());
        } else if (data.getData() != null) {
            uris.add(data.getData());
        }
        new Thread(() -> copyAll(uris), "copy-import").start();
    }

    /**
     * Copies the picked documents into files/import/.incoming/, then moves them all to
     * files/import/ at once, so a .cue never shows up before its .bin. Progress goes to
     * files/import/.status (read by the native import screen).
     */
    private void copyAll(ArrayList<Uri> uris) {
        File base = getExternalFilesDir(null);
        if (base == null) base = getFilesDir();
        File importDir = new File(base, "import");
        File incoming = new File(importDir, ".incoming");
        incoming.mkdirs();
        ArrayList<File> done = new ArrayList<>();
        byte[] buf = new byte[1 << 20];
        try {
            for (Uri u : uris) {
                String name = displayName(u);
                long size = size(u);
                File out = new File(incoming, name);
                try (InputStream in = getContentResolver().openInputStream(u); OutputStream os = new FileOutputStream(out)) {
                    long copied = 0, last = 0;
                    int n;
                    while ((n = in.read(buf)) > 0) {
                        os.write(buf, 0, n);
                        copied += n;
                        if (copied - last >= (8 << 20)) {
                            last = copied;
                            status("copy " + name + " " + copied + " " + size);
                        }
                    }
                }
                done.add(out);
            }
            for (File f : done) {
                File dst = new File(importDir, f.getName());
                if (!f.renameTo(dst)) throw new java.io.IOException("cannot move " + f + " to " + dst);
            }
            status("copied " + done.size());
        } catch (Exception e) {
            Log.e(TAG, "copy failed: " + e);
            for (File f : done) f.delete();
            status("error " + e.getMessage());
        }
    }

    private void status(String s) {
        File base = getExternalFilesDir(null);
        if (base == null) base = getFilesDir();
        File dir = new File(base, "import");
        dir.mkdirs();
        try (OutputStream os = new FileOutputStream(new File(dir, ".status"))) {
            os.write(s.getBytes("UTF-8"));
        } catch (Exception e) {
            Log.e(TAG, "status: " + e);
        }
    }

    private String displayName(Uri u) {
        String name = null;
        try (Cursor c = getContentResolver().query(u, new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (c != null && c.moveToFirst()) name = c.getString(0);
        } catch (Exception e) {
            // fall through
        }
        if (name == null) name = u.getLastPathSegment();
        if (name == null) name = "picked.bin";
        return name.replaceAll("[/\\\\:]", "_");
    }

    private long size(Uri u) {
        try (Cursor c = getContentResolver().query(u, new String[] {OpenableColumns.SIZE}, null, null, null)) {
            if (c != null && c.moveToFirst() && !c.isNull(0)) return c.getLong(0);
        } catch (Exception e) {
            // unknown
        }
        return -1;
    }
}
