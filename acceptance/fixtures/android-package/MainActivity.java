package com.pixels.acceptance.packagefixture;

import android.app.Activity;
import android.os.Bundle;
import android.widget.TextView;
import java.util.UUID;

/** An owned, permission-free package used only for remote install/update/remove acceptance. */
public final class MainActivity extends Activity {
  @Override
  public void onCreate(Bundle savedInstanceState) {
    super.onCreate(savedInstanceState);
    var preferences = getSharedPreferences("retention", MODE_PRIVATE);
    String token = preferences.getString("token", null);
    if (token == null) {
      token = UUID.randomUUID().toString();
      preferences.edit().putString("token", token).commit();
    }
    long version;
    try {
      version = getPackageManager().getPackageInfo(getPackageName(), 0).getLongVersionCode();
    } catch (Exception failure) {
      throw new IllegalStateException(failure);
    }
    TextView view = new TextView(this);
    view.setTextSize(20);
    view.setPadding(24, 48, 24, 24);
    view.setText("Pixels package fixture\nVersion: " + version + "\nRetained: " + token);
    setContentView(view);
  }
}
