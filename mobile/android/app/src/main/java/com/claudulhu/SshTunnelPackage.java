package com.claudulhu;

import com.facebook.react.TurboReactPackage;
import com.facebook.react.bridge.NativeModule;
import com.facebook.react.bridge.ReactApplicationContext;
import com.facebook.react.module.model.ReactModuleInfo;
import com.facebook.react.module.model.ReactModuleInfoProvider;

import java.util.HashMap;
import java.util.Map;

import androidx.annotation.NonNull;
import androidx.annotation.Nullable;

/**
 * React Native package that exposes {@link SshTunnelModule}.
 *
 * With New Architecture enabled, modules that implement a codegen-generated
 * spec (NativeSshTunnelSpec) are resolved via {@link TurboReactPackage}'s
 * module provider mechanism, so no explicit add() in MainApplication is
 * required when using autolinking.  This class is still needed so that the
 * module can be discovered by the React Native infrastructure.
 */
public class SshTunnelPackage extends TurboReactPackage {

    @Nullable
    @Override
    public NativeModule getModule(
            @NonNull String name,
            @NonNull ReactApplicationContext reactContext) {
        if (SshTunnelModule.NAME.equals(name)) {
            return new SshTunnelModule(reactContext);
        }
        return null;
    }

    @Override
    public ReactModuleInfoProvider getReactModuleInfoProvider() {
        return () -> {
            Map<String, ReactModuleInfo> map = new HashMap<>();
            map.put(
                SshTunnelModule.NAME,
                new ReactModuleInfo(
                    SshTunnelModule.NAME,   // name
                    SshTunnelModule.NAME,   // className
                    false,                  // canOverrideExistingModule
                    false,                  // needsEagerInit
                    false,                  // isCxxModule — false for Java TurboModules
                    true                    // isTurboModule
                )
            );
            return map;
        };
    }
}
