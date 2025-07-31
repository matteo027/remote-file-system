import express from 'express';
import { setRoutes as setRoutesFS } from './routes/filesystemRoutes';
import { setRoutes as setRoutesAuth } from './routes/authenticationRoutes';
import { AppDataSource } from './data-source';
import { User as FSUser } from './entities/User';
import passport from 'passport';
import session from 'express-session';
import { Strategy as LocalStrategy } from 'passport-local';
import cors from 'cors';
import { AuthenticationController } from './controllers/authenticationController';
import { File } from './entities/File';
import { promises as fs } from 'fs';
import path from 'path';

const app = express();
const PORT = process.env.PORT || 3000;

app.use(express.json());

app.listen(PORT, () => {
    console.log(`Server is running on http://localhost:${PORT}`);
});

app.use(passport.initialize());

// session in express
app.use(session({
  secret: "shh",
  resave: false,
  saveUninitialized: false
}));
app.use(passport.authenticate('session'));

passport.use(new LocalStrategy(
  async function verify(username: string, password: string, cb) {
    try {
      if([...username].some(c => c < '0' || c > '9')) return cb(null, false, { message: "Incorrect username or password." });
      const uid = parseInt(username);
      const user = await new AuthenticationController().getUser(uid, password);
      if (!user) {
        return cb(null, false, { message: "Incorrect username or password." });
      }
      return cb(null, user);
    } catch (err) {
      return cb(err);
    }
}));


passport.serializeUser((user: any, done) => {
  done(null, (user as FSUser).uid);
});


passport.deserializeUser(async (username: string, done) => {
  try {
    const user = await AppDataSource.getRepository(FSUser).findOneBy({ uid: parseInt(username) });
    done(null, user || false);
  } catch (err) {
    done(err);
  }
});

const corsOptions = {
  origin: 'https://localhost:3000',
  credentials: true,
};
app.use(cors(corsOptions));


// Set up routes
setRoutesFS(app);
setRoutesAuth(app);

// initializing the db
async function db() {
  try {
    await AppDataSource.initialize();
    console.log("Data Source has been initialized and DB schema created.");

    const userRepo = AppDataSource.getRepository(FSUser);
    const fileRepo = AppDataSource.getRepository(File);
    const exists = await userRepo.findOneBy({ uid: 5000 });

    if (!exists) {
      const admin = userRepo.create({
        uid: 5000,
        password: "c7be23ada64b3748d4a0aba3604a305535e757f69e5ca67726f013f8303b90fc", // hashed "admin"
        salt: "d610f867285f3cd63aa5ee46e9e1de55"
      });
      await userRepo.save(admin);
      
      
      // creating the 5000 (admin) folder
      await fs.mkdir('./file-system/5000', { recursive: true });
      let now = Date.now();
      const admin_dir = fileRepo.create({
        path: '/5000',
        owner: admin,
        type: 1,
        permissions: 0o755,
        atime: now,
        btime: now,
        ctime: now,
        mtime: now,
      } as File);
      const root_dir = fileRepo.create({
        path: '/',
        owner: admin,
        type: 1,
        permissions: 0o755,
        atime: now,
        btime: now,
        ctime: now,
        mtime: now,
      } as File);
      fileRepo.save(admin_dir);
      fileRepo.save(root_dir);
    }

  } catch (error) {
    console.error("Error during Data Source initialization: ", error);
  }
}

db();