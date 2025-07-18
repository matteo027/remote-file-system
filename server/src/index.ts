import express, { Request } from 'express';
import { setRoutes } from './routes/filesystemRoutes';
import { AppDataSource } from './data-source';
import { User as FSUser } from './entities/User';
import * as passportStrategy from 'passport-local';
import passport from 'passport';
import session from 'express-session';
import { FileSystemController } from './controllers/filesystemController';
import { Strategy as LocalStrategy } from 'passport-local';

const app = express();
const PORT = process.env.PORT || 3000;

// Middleware
app.use(express.json());

// Set up routes
setRoutes(app);

app.listen(PORT, () => {
    console.log(`Server is running on http://localhost:${PORT}`);
});

app.use(passport.initialize());

app.use(session({
  secret: "",
  resave: false,
  saveUninitialized: false
}));
app.use(passport.authenticate('session'));

passport.use(new LocalStrategy(
  async function verify(username: string, password: string, cb) {
    try {
      const user = await new FileSystemController().getUser(username, password);
      if (!user) {
        return cb(null, false, { message: "Incorrect username or password." });
      }
      return cb(null, user);
    } catch (err) {
      return cb(err);
    }
}));


passport.serializeUser((user: any, done) => { // it should be FSUser btw
  done(null, user.username);
});


passport.deserializeUser(async (username: string, done) => {
  try {
    const user = await AppDataSource.getRepository(FSUser).findOneBy({ username });
    done(null, user || false);
  } catch (err) {
    done(err);
  }
});


// initializing the db
async function db() {
  try {
    await AppDataSource.initialize();
    console.log("Data Source has been initialized and DB schema created.");

    const userRepo = AppDataSource.getRepository(FSUser);
    const exists = await userRepo.findOneBy({ username: "admin" });

    if (!exists) {
      const admin = userRepo.create({
        username: "admin",
        password: "admin",
      });

      await userRepo.save(admin);
    }

  } catch (error) {
    console.error("Error during Data Source initialization:", error);
  }
}

db();